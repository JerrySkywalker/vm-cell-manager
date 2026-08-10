use std::error::Error;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, error::ErrorKind};
use serde::Serialize;
use vm_cell_manager::cli::{
    ArtifactCommand, Cli, CliExitCode, CliInputError, CliProvider, Command, CredentialArgs,
    DoctorReport, ErrorEnvelope, GuestOperationCommand, ImageCommand, ListEnvelope,
    ProviderCommand, RunErrorEnvelope, StatusCellEntry, StatusCellObservation,
    StatusCleanupGuidance, StatusImageEntry, StatusImageObservation, StatusImageVariantObservation,
    StatusOperationEntry, StatusReport, StatusRetention, classify_cli_error, public_error_message,
};
use vm_cell_manager::core::automation::RequiredAction;
use vm_cell_manager::core::cell::{CellPhase, CellRecord, CellSpec, CellState};
use vm_cell_manager::core::guest::{
    GuestFailureClass, GuestOperationKind, GuestOperationPhase, GuestOperationRecord,
};
use vm_cell_manager::core::image::{Architecture, GuestOs, ImageRecord};
use vm_cell_manager::engine::{
    ArtifactCollectRequest, ArtifactPruneRequest, CellEngine, CellInspection, EngineError,
    GuestCopyInRequest, GuestCopyOutRequest, GuestExecRequest, GuestOperationRecoveryReport,
    ImageValidationReport, ImageValidationStatus, RegisterImageRequest, RunCellError,
    RunCellReport, RunCellRequest, RunCleanupPolicy, RunControl, RunObserver, RunProgressEvent,
    ValidateImageRequest,
};
use vm_cell_manager::guest::powershell_direct::PowerShellDirectTransport;
use vm_cell_manager::guest::qga::QemuGuestAgentTransport;
use vm_cell_manager::guest::{GuestCommand, GuestCredentials, ReadinessPolicy};
use vm_cell_manager::providers::hyperv::HyperVProvider;
use vm_cell_manager::providers::qemu::QemuProvider;
use vm_cell_manager::providers::{
    LocalVmProvider, ProviderPowerState, ProviderProbe, ProviderProbeStatus,
    builtin_provider_probes,
};
use vm_cell_manager::state::StateStore;
use zeroize::Zeroizing;

fn main() -> ExitCode {
    let json_requested = json_requested_by_argv();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return emit_parse_error(error, json_requested),
    };
    let json = cli.json;
    match run(cli) {
        Ok(exit_code) => exit_code,
        Err(error) => emit_error(error.as_ref(), json),
    }
}

fn emit_error(error: &(dyn Error + 'static), json: bool) -> ExitCode {
    let classification = classify_cli_error(error);
    if let Some(run_error) = error.downcast_ref::<RunCellError>() {
        return emit_run_error(classification, run_error, json);
    }
    emit_classified_error(classification, json)
}

fn emit_run_error(
    classification: vm_cell_manager::cli::CliErrorClassification,
    error: &RunCellError,
    json: bool,
) -> ExitCode {
    let message = public_error_message(classification);
    if json {
        let envelope = RunErrorEnvelope::new(classification, message, error.report());
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&envelope)
                .unwrap_or_else(|_| "{\"schema_version\":1,\"error\":{\"code\":\"vmcell.internal\",\"category\":\"internal\",\"message\":\"error serialization failed\",\"retryable\":false,\"exit_code\":10}}".to_owned())
        );
    } else {
        let report = error.report();
        let cell = report
            .cell_id
            .map_or_else(|| "none".to_owned(), |cell_id| cell_id.to_string());
        let operation = report.operation_id.map_or_else(
            || "none".to_owned(),
            |operation_id| operation_id.to_string(),
        );
        let cleanup_error = report.cleanup_error_code.as_deref().unwrap_or("none");
        eprintln!(
            "vmcell: {}: {message}; run stage={} cell={} operation={} cleanup={} cleanup_error={}",
            classification.code,
            report.stage.as_str(),
            cell,
            operation,
            report.cleanup.as_str(),
            cleanup_error
        );
    }
    ExitCode::from(classification.exit_code.as_u8())
}

fn json_requested_by_argv() -> bool {
    std::env::args_os()
        .skip(1)
        .take_while(|argument| argument != "--")
        .any(|argument| argument == "--json")
}

fn emit_parse_error(error: clap::Error, json_requested: bool) -> ExitCode {
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        let _ = error.print();
        return ExitCode::SUCCESS;
    }
    if json_requested {
        let classification =
            classify_cli_error(&CliInputError("argument parsing failed".to_owned()));
        emit_classified_error(classification, true)
    } else {
        let _ = error.print();
        ExitCode::from(2)
    }
}

fn emit_classified_error(
    classification: vm_cell_manager::cli::CliErrorClassification,
    json: bool,
) -> ExitCode {
    let message = public_error_message(classification);
    if json {
        let envelope = ErrorEnvelope::new(classification, message);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&envelope)
                .unwrap_or_else(|_| "{\"schema_version\":1,\"error\":{\"code\":\"vmcell.internal\",\"category\":\"internal\",\"message\":\"error serialization failed\",\"retryable\":false,\"exit_code\":10}}".to_owned())
        );
    } else {
        eprintln!("vmcell: {}: {message}", classification.code);
    }
    ExitCode::from(classification.exit_code.as_u8())
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn Error>> {
    let state_root = cli.state_root.clone();
    let lock_timeout = Duration::from_millis(cli.lock_timeout_ms);
    match cli.command {
        Command::Doctor => {
            let report = DoctorReport::collect(state_root);
            emit(&report, cli.json, || {
                write_doctor_report(&report, &mut std::io::stdout().lock())
                    .expect("stdout should remain writable");
            })?;
        }
        Command::Status => {
            let report = collect_status(
                state_root.unwrap_or_else(StateStore::default_root),
                lock_timeout,
            )?;
            emit(&report, cli.json, || {
                write_status_report(&report, &mut std::io::stdout().lock())
                    .expect("stdout should remain writable");
            })?;
        }
        Command::Provider {
            command: ProviderCommand::List,
        } => {
            let response = ListEnvelope::new(builtin_provider_probes());
            emit(&response, cli.json, || {
                for probe in &response.items {
                    write_provider_probe(probe, &mut std::io::stdout().lock())
                        .expect("stdout should remain writable");
                }
            })?;
        }
        Command::Reconcile { cell_id: None } => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let mut items = CellEngine::new(
                StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout),
                HyperVProvider::system(),
            )
            .reconcile_all()?;
            items.extend(
                CellEngine::new(
                    StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout),
                    QemuProvider::system(root),
                )
                .reconcile_all()?,
            );
            let response = ListEnvelope::new(items);
            emit(&response, cli.json, || {
                for inspection in &response.items {
                    write_cell_inspection(
                        inspection,
                        &[],
                        chrono::Utc::now(),
                        &mut std::io::stdout().lock(),
                    )
                    .expect("stdout should remain writable");
                }
            })?;
        }
        Command::Gc => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let evaluated_at = chrono::Utc::now();
            let mut report = CellEngine::new(
                StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout),
                HyperVProvider::system(),
            )
            .gc_expired_at(evaluated_at)?;
            report.entries.extend(
                CellEngine::new(
                    StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout),
                    QemuProvider::system(root),
                )
                .gc_expired_at(evaluated_at)?
                .entries,
            );
            emit(&report, cli.json, || {
                for entry in &report.entries {
                    println!("{}\t{:?}", entry.cell_id, entry.disposition);
                }
            })?;
        }
        command => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let state = StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout);
            let provider = provider_for_command(&command, &state)?;
            return match provider.as_str() {
                "hyperv" => run_m2(
                    command,
                    cli.json,
                    &CellEngine::new(state, HyperVProvider::system()),
                ),
                "qemu" => run_m2(
                    command,
                    cli.json,
                    &CellEngine::new(state, QemuProvider::system(root)),
                ),
                value => Err(EngineError::Integrity(format!(
                    "unsupported persisted provider: {value}"
                ))
                .into()),
            };
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run_m2<P: LocalVmProvider>(
    command: Command,
    json: bool,
    engine: &CellEngine<P>,
) -> Result<ExitCode, Box<dyn Error>> {
    match command {
        Command::Image { command } => match command {
            ImageCommand::Add {
                id,
                path,
                guest_os,
                guest_arch,
                provider: _,
            } => {
                let image = engine.register_image(RegisterImageRequest {
                    id,
                    guest_os: guest_os.into(),
                    guest_arch: guest_arch.into(),
                    path,
                })?;
                emit(&image, json, || {
                    let mut stdout = std::io::stdout().lock();
                    write_registered_image(&image, &mut stdout)
                        .expect("stdout should remain writable");
                })?;
            }
            ImageCommand::Validate {
                id,
                path,
                guest_os,
                guest_arch,
                provider: _,
            } => {
                let report = if let Some(id) = id {
                    engine.validate_registered_image(&id)?
                } else {
                    engine.validate_image(ValidateImageRequest {
                        guest_os: guest_os
                            .ok_or_else(|| CliInputError("--guest-os is required".to_owned()))?
                            .into(),
                        guest_arch: guest_arch.into(),
                        path: path.ok_or_else(|| CliInputError("--path is required".to_owned()))?,
                    })?
                };
                emit(&report, json, || {
                    let mut stdout = std::io::stdout().lock();
                    write_image_validation(&report, &mut stdout)
                        .expect("stdout should remain writable");
                })?;
                if report.status == ImageValidationStatus::Unusable {
                    return Ok(ExitCode::from(CliExitCode::Integrity.as_u8()));
                }
            }
            ImageCommand::List => {
                let response = ListEnvelope::new(engine.list_images()?);
                emit(&response, json, || {
                    for image in &response.items {
                        let mut stdout = std::io::stdout().lock();
                        write_registered_image(image, &mut stdout)
                            .expect("stdout should remain writable");
                    }
                })?;
            }
            ImageCommand::Inspect { id } => {
                let image = engine.inspect_image(&id)?;
                if json {
                    emit(&image, true, || {})?;
                } else {
                    let report = engine.validate_registered_image(&id)?;
                    let mut stdout = std::io::stdout().lock();
                    write_registered_image(&image, &mut stdout)?;
                    write_image_validation(&report, &mut stdout)?;
                    if report.status == ImageValidationStatus::Unusable {
                        return Ok(ExitCode::from(CliExitCode::Integrity.as_u8()));
                    }
                }
            }
        },
        Command::Create {
            image,
            cpu_count,
            memory_mib,
            ttl_seconds,
            provider: _,
            accelerator,
            allow_tcg,
        } => {
            let cell = engine.create_cell(CellSpec {
                image,
                provider: Some(engine.provider_name().to_owned()),
                cpu_count,
                memory_mib,
                ttl_seconds,
                accelerator: accelerator.map(|value| value.as_str().to_owned()),
                allow_tcg,
            })?;
            emit(&cell, json, || {
                println!("created cell {} ({:?})", cell.id, cell.state)
            })?;
        }
        Command::Run {
            image,
            cpu_count,
            memory_mib,
            ttl_seconds,
            provider: _,
            accelerator,
            allow_tcg,
            keep,
            keep_on_failure,
            credential,
            readiness_timeout_seconds,
            action_timeout_seconds,
            max_output_bytes,
            mut command,
        } => {
            let program = command.remove(0);
            let request = RunCellRequest {
                spec: CellSpec {
                    image,
                    provider: Some(engine.provider_name().to_owned()),
                    cpu_count,
                    memory_mib,
                    ttl_seconds,
                    accelerator: accelerator.map(|value| value.as_str().to_owned()),
                    allow_tcg,
                },
                command: GuestCommand {
                    program,
                    args: command,
                    timeout: Duration::from_secs(action_timeout_seconds),
                    max_output_bytes,
                },
                readiness: readiness(readiness_timeout_seconds),
                cleanup: RunCleanupPolicy {
                    keep,
                    keep_on_failure,
                },
            };
            let report = if json {
                let mut observer = |_event: &RunProgressEvent| RunControl::Continue;
                run_cell_guest(engine, credential, request, &mut observer)?
            } else {
                let mut observer = HumanRunObserver::new(std::io::stderr());
                let result = run_cell_guest(engine, credential, request, &mut observer);
                let output_result = observer.finish().map(|_| ());
                let report = result?;
                output_result?;
                report
            };
            if json {
                emit(&report, true, || {})?;
            } else {
                let stdout = std::io::stdout();
                let stderr = std::io::stderr();
                write_human_run_result(&report, &mut stdout.lock(), &mut stderr.lock())?;
            }
            return Ok(guest_exit_status(report.result.exit_code));
        }
        Command::List => {
            let response = ListEnvelope::new(engine.list_cells()?);
            emit(&response, json, || {
                for cell in &response.items {
                    write_cell_summary(cell, chrono::Utc::now(), &mut std::io::stdout().lock())
                        .expect("stdout should remain writable");
                }
            })?;
        }
        Command::Inspect { cell_id } => {
            let inspection = engine.inspect_cell(cell_id)?;
            let operations = if json {
                Vec::new()
            } else {
                engine.list_guest_operations(Some(cell_id))?
            };
            emit(&inspection, json, || {
                write_cell_inspection(
                    &inspection,
                    &operations,
                    chrono::Utc::now(),
                    &mut std::io::stdout().lock(),
                )
                .expect("stdout should remain writable");
            })?;
        }
        Command::Start { cell_id } => {
            let report = engine.start_cell(cell_id)?;
            emit(&report, json, || {
                println!("started cell {}", report.cell_id)
            })?;
        }
        Command::Stop { cell_id } => {
            let report = engine.stop_cell(cell_id)?;
            emit(&report, json, || {
                println!("stopped cell {}", report.cell_id)
            })?;
        }
        Command::Destroy { cell_id } => {
            let report = engine.destroy_cell(cell_id)?;
            emit(&report, json, || {
                println!("destroyed cell {}", report.cell_id)
            })?;
        }
        Command::Reconcile { cell_id } => {
            if let Some(cell_id) = cell_id {
                let inspection = engine.reconcile_cell(cell_id)?;
                let operations = if json {
                    Vec::new()
                } else {
                    engine.list_guest_operations(Some(cell_id))?
                };
                emit(&inspection, json, || {
                    write_cell_inspection(
                        &inspection,
                        &operations,
                        chrono::Utc::now(),
                        &mut std::io::stdout().lock(),
                    )
                    .expect("stdout should remain writable");
                })?;
            } else {
                let response = ListEnvelope::new(engine.reconcile_all()?);
                emit(&response, json, || {
                    for inspection in &response.items {
                        write_cell_inspection(
                            inspection,
                            &[],
                            chrono::Utc::now(),
                            &mut std::io::stdout().lock(),
                        )
                        .expect("stdout should remain writable");
                    }
                })?;
            }
        }
        Command::Exec {
            cell_id,
            credential,
            readiness_timeout_seconds,
            timeout_seconds,
            max_output_bytes,
            mut command,
        } => {
            let program = command.remove(0);
            let report = exec_guest(
                engine,
                credential,
                GuestExecRequest {
                    cell_id,
                    command: GuestCommand {
                        program,
                        args: command,
                        timeout: Duration::from_secs(timeout_seconds),
                        max_output_bytes,
                    },
                    readiness: readiness(readiness_timeout_seconds),
                },
            )?;
            emit(&report, json, || {
                println!("guest exec {}", report.operation_id)
            })?;
        }
        Command::CopyIn {
            cell_id,
            source,
            destination,
            overwrite,
            credential,
            readiness_timeout_seconds,
            timeout_seconds,
            max_bytes,
        } => {
            let report = copy_in_guest(
                engine,
                credential,
                GuestCopyInRequest {
                    cell_id,
                    source,
                    destination,
                    overwrite: overwrite.into(),
                    timeout: Duration::from_secs(timeout_seconds),
                    max_bytes,
                    readiness: readiness(readiness_timeout_seconds),
                },
            )?;
            emit(&report, json, || {
                println!("copied into guest {}", report.operation_id)
            })?;
        }
        Command::CopyOut {
            cell_id,
            source,
            credential,
            readiness_timeout_seconds,
            timeout_seconds,
            max_bytes,
        } => {
            let report = copy_out_guest(
                engine,
                credential,
                GuestCopyOutRequest {
                    cell_id,
                    source,
                    timeout: Duration::from_secs(timeout_seconds),
                    max_bytes,
                    readiness: readiness(readiness_timeout_seconds),
                },
            )?;
            emit(&report, json, || {
                println!("artifact {}", report.operation_id)
            })?;
        }
        Command::Artifact { command } => match command {
            ArtifactCommand::Collect {
                cell_id,
                paths,
                credential,
                readiness_timeout_seconds,
                timeout_seconds,
                max_bytes_per_file,
            } => {
                let report = collect_guest_artifacts(
                    engine,
                    credential,
                    ArtifactCollectRequest {
                        cell_id,
                        sources: paths,
                        timeout: Duration::from_secs(timeout_seconds),
                        max_bytes_per_file,
                        readiness: readiness(readiness_timeout_seconds),
                    },
                )?;
                emit(&report, json, || {
                    println!("artifact {}", report.operation_id)
                })?;
            }
            ArtifactCommand::Inspect {
                cell_id,
                operation_id,
            } => {
                let artifact = engine.inspect_artifact(cell_id, operation_id)?;
                emit(&artifact, json, || println!("{artifact:#?}"))?;
            }
            ArtifactCommand::Prune {
                older_than_seconds,
                max_artifacts,
                dry_run,
            } => {
                let report = engine.prune_artifacts(ArtifactPruneRequest {
                    older_than: Duration::from_secs(older_than_seconds),
                    max_artifacts,
                    dry_run,
                })?;
                emit(&report, json, || {
                    for entry in &report.entries {
                        println!(
                            "{}\t{}\t{:?}",
                            entry.cell_id, entry.operation_id, entry.disposition
                        );
                    }
                })?;
            }
        },
        Command::Operation { command } => match command {
            GuestOperationCommand::List { cell_id } => {
                let response = ListEnvelope::new(engine.list_guest_operations(cell_id)?);
                emit(&response, json, || {
                    for operation in &response.items {
                        write_guest_operation(operation, &mut std::io::stdout().lock())
                            .expect("stdout should remain writable");
                    }
                })?;
            }
            GuestOperationCommand::Inspect { operation_id } => {
                let operation = engine.inspect_guest_operation(operation_id)?;
                emit(&operation, json, || {
                    write_guest_operation(&operation, &mut std::io::stdout().lock())
                        .expect("stdout should remain writable");
                })?;
            }
            GuestOperationCommand::Reconcile { operation_id } => {
                let report = engine.reconcile_guest_operation(operation_id)?;
                emit(&report, json, || {
                    write_guest_operation_recovery(&report, &mut std::io::stdout().lock())
                        .expect("stdout should remain writable");
                })?;
            }
        },
        Command::Gc => {
            let report = engine.gc_expired()?;
            emit(&report, json, || {
                for entry in &report.entries {
                    println!("{}\t{:?}", entry.cell_id, entry.disposition);
                }
            })?;
        }
        Command::Doctor | Command::Status | Command::Provider { .. } => {
            unreachable!("handled before engine creation")
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn collect_status(
    root: std::path::PathBuf,
    lock_timeout: Duration,
) -> Result<StatusReport, Box<dyn Error>> {
    let evaluated_at = chrono::Utc::now();
    let providers = builtin_provider_probes();
    let state = StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout);
    let operations = state.list_guest_operations()?;
    let operation_entries = operations
        .iter()
        .cloned()
        .map(|operation| StatusOperationEntry {
            pending: !operation.phase.is_terminal(),
            uncertain: operation.phase == GuestOperationPhase::TransportActive,
            required_action: guest_operation_required_action(operation.phase),
            operation,
        })
        .collect::<Vec<_>>();

    let mut cells = Vec::new();
    for cell in state.list_cells()? {
        let pending_operations = operations
            .iter()
            .filter(|operation| operation.cell_id == cell.id && !operation.phase.is_terminal())
            .count();
        let uncertain_operations = operations
            .iter()
            .filter(|operation| {
                operation.cell_id == cell.id
                    && operation.phase == GuestOperationPhase::TransportActive
            })
            .count();
        let retention = status_retention(&cell, evaluated_at);
        let provider_status = status_provider_status(&providers, &cell.provider);
        let (observation, cleanup) = if provider_status != ProviderProbeStatus::Ready {
            (
                StatusCellObservation::ProviderUnavailable { provider_status },
                StatusCleanupGuidance::ManualReview,
            )
        } else {
            match inspect_status_cell(&root, lock_timeout, &cell) {
                Ok(inspection) => {
                    let cleanup = if pending_operations > 0 {
                        StatusCleanupGuidance::ManualReview
                    } else {
                        status_cleanup_guidance(&inspection)
                    };
                    (
                        StatusCellObservation::Observed {
                            inspection: Box::new(inspection),
                        },
                        cleanup,
                    )
                }
                Err(error) => (
                    StatusCellObservation::InspectionFailed {
                        error_code: classify_cli_error(&error).code.to_owned(),
                    },
                    StatusCleanupGuidance::ManualReview,
                ),
            }
        };
        cells.push(StatusCellEntry {
            cell,
            retention,
            pending_operations,
            uncertain_operations,
            observation,
            cleanup,
        });
    }

    let mut images = Vec::new();
    for image in state.list_images()? {
        let mut observations = Vec::new();
        for variant in &image.variants {
            let provider = &variant.provider;
            let provider_status = status_provider_status(&providers, provider);
            let observation = if provider_status != ProviderProbeStatus::Ready {
                StatusImageObservation::ProviderUnavailable { provider_status }
            } else {
                match validate_status_image(&root, lock_timeout, &image, provider) {
                    Ok(report) => StatusImageObservation::Validated {
                        report: Box::new(report),
                    },
                    Err(error) => StatusImageObservation::ValidationFailed {
                        error_code: classify_cli_error(&error).code.to_owned(),
                    },
                }
            };
            observations.push(StatusImageVariantObservation {
                provider: provider.clone(),
                observation,
            });
        }
        observations.sort_by(|left, right| left.provider.cmp(&right.provider));
        images.push(StatusImageEntry {
            image,
            observations,
        });
    }

    Ok(StatusReport::new(
        evaluated_at,
        providers,
        cells,
        images,
        operation_entries,
    ))
}

fn inspect_status_cell(
    root: &std::path::Path,
    lock_timeout: Duration,
    cell: &CellRecord,
) -> Result<CellInspection, EngineError> {
    let state = || StateStore::new(root.to_path_buf()).with_mutation_lock_timeout(lock_timeout);
    match cell.provider.as_str() {
        "hyperv" => CellEngine::new(state(), HyperVProvider::system()).inspect_cell(cell.id),
        "qemu" => {
            CellEngine::new(state(), QemuProvider::system(root.to_path_buf())).inspect_cell(cell.id)
        }
        provider => Err(EngineError::Integrity(format!(
            "unsupported persisted provider: {provider}"
        ))),
    }
}

fn validate_status_image(
    root: &std::path::Path,
    lock_timeout: Duration,
    image: &ImageRecord,
    provider: &str,
) -> Result<ImageValidationReport, EngineError> {
    let state = || StateStore::new(root.to_path_buf()).with_mutation_lock_timeout(lock_timeout);
    match provider {
        "hyperv" => {
            CellEngine::new(state(), HyperVProvider::system()).validate_registered_image(&image.id)
        }
        "qemu" => CellEngine::new(state(), QemuProvider::system(root.to_path_buf()))
            .validate_registered_image(&image.id),
        provider => Err(EngineError::ImageIntegrity(format!(
            "unsupported persisted image provider: {provider}"
        ))),
    }
}

fn status_provider_status(providers: &[ProviderProbe], provider: &str) -> ProviderProbeStatus {
    providers
        .iter()
        .find(|probe| probe.name == provider)
        .map_or(ProviderProbeStatus::ProbeFailed, |probe| probe.status)
}

fn status_retention(
    cell: &CellRecord,
    evaluated_at: chrono::DateTime<chrono::Utc>,
) -> StatusRetention {
    if cell.state == CellState::Destroyed {
        StatusRetention::None
    } else {
        match cell.expires_at {
            Some(expires_at) if expires_at <= evaluated_at => StatusRetention::Expired,
            Some(_) => StatusRetention::ActiveUntilExpiry,
            None => StatusRetention::Manual,
        }
    }
}

fn status_cleanup_guidance(inspection: &CellInspection) -> StatusCleanupGuidance {
    match inspection.classification.required_action {
        RequiredAction::None if inspection.cell.state == CellState::Destroyed => {
            StatusCleanupGuidance::NotNeeded
        }
        RequiredAction::None => StatusCleanupGuidance::ExactOwnedDestroy,
        RequiredAction::RetryLifecycle => StatusCleanupGuidance::ReconcileThenRetry,
        RequiredAction::RecoveryRequired => StatusCleanupGuidance::PhaseRecovery,
        RequiredAction::ManualReview => StatusCleanupGuidance::ManualReview,
    }
}

fn guest_operation_required_action(phase: GuestOperationPhase) -> RequiredAction {
    match phase {
        GuestOperationPhase::IntentRecorded | GuestOperationPhase::ArtifactCommitted => {
            RequiredAction::RecoveryRequired
        }
        GuestOperationPhase::TransportActive => RequiredAction::ManualReview,
        GuestOperationPhase::Completed | GuestOperationPhase::Failed => RequiredAction::None,
    }
}

fn provider_for_command(command: &Command, state: &StateStore) -> Result<String, Box<dyn Error>> {
    let provider = match command {
        Command::Image {
            command: ImageCommand::Add { provider, .. } | ImageCommand::Validate { provider, .. },
        }
        | Command::Create { provider, .. }
        | Command::Run { provider, .. } => provider.as_str().to_owned(),
        Command::Inspect { cell_id }
        | Command::Start { cell_id }
        | Command::Stop { cell_id }
        | Command::Destroy { cell_id }
        | Command::Exec { cell_id, .. }
        | Command::CopyIn { cell_id, .. }
        | Command::CopyOut { cell_id, .. }
        | Command::Reconcile {
            cell_id: Some(cell_id),
        } => state.load_cell(*cell_id)?.provider,
        Command::Artifact {
            command:
                ArtifactCommand::Collect { cell_id, .. } | ArtifactCommand::Inspect { cell_id, .. },
        } => state.load_cell(*cell_id)?.provider,
        Command::Artifact {
            command: ArtifactCommand::Prune { .. },
        } => CliProvider::Hyperv.as_str().to_owned(),
        Command::Image {
            command: ImageCommand::Inspect { id },
        } => {
            let image = state.load_image(id)?;
            if image.variants.len() != 1 {
                return Err(EngineError::ImageIntegrity(
                    "registered image does not contain exactly one provider variant".to_owned(),
                )
                .into());
            }
            image.variants[0].provider.clone()
        }
        Command::List
        | Command::Image {
            command: ImageCommand::List,
        }
        | Command::Operation { .. }
        | Command::Doctor
        | Command::Status
        | Command::Provider { .. } => CliProvider::Hyperv.as_str().to_owned(),
        Command::Reconcile { cell_id: None } | Command::Gc => {
            return Err("multi-provider command was not routed before provider selection".into());
        }
    };
    Ok(provider)
}

fn exec_guest<P: LocalVmProvider>(
    engine: &CellEngine<P>,
    credential: CredentialArgs,
    request: GuestExecRequest,
) -> Result<vm_cell_manager::engine::GuestExecReport, Box<dyn Error>> {
    let credentials = read_credentials(engine.provider_name(), credential)?;
    Ok(match engine.provider_name() {
        "hyperv" => {
            engine.exec_guest(&PowerShellDirectTransport::system(), &credentials, request)?
        }
        "qemu" => engine.exec_guest(&QemuGuestAgentTransport::system(), &credentials, request)?,
        value => {
            return Err(
                EngineError::Integrity(format!("unsupported guest provider: {value}")).into(),
            );
        }
    })
}

fn write_registered_image(image: &ImageRecord, output: &mut impl Write) -> std::io::Result<()> {
    writeln!(
        output,
        "image={} guest_os={} guest_arch={} registered_at={}",
        image.id,
        guest_os_name(image.guest_os),
        architecture_name(image.guest_arch),
        image.registered_at.to_rfc3339()
    )?;
    for variant in &image.variants {
        writeln!(
            output,
            "  provider={} format={} size={} sha256={} path={}",
            variant.provider,
            variant.disk_format,
            variant.file_size,
            variant.sha256,
            variant.path.display()
        )?;
    }
    Ok(())
}

fn write_image_validation(
    report: &ImageValidationReport,
    output: &mut impl Write,
) -> std::io::Result<()> {
    let image = report
        .image_id
        .as_ref()
        .map_or("none", |image_id| image_id.as_str());
    let observed_format = report.observed_format.as_deref().unwrap_or("unknown");
    let sha256 = report.sha256.as_deref().unwrap_or("unavailable");
    let parent = report
        .parent_path
        .as_ref()
        .map_or_else(|| "none".to_owned(), |path| path.display().to_string());
    let issues = if report.issues.is_empty() {
        "none".to_owned()
    } else {
        report
            .issues
            .iter()
            .map(|issue| issue.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    writeln!(
        output,
        "validation={} image={} registered={} provider={} guest_os={} guest_arch={}",
        report.status.as_str(),
        image,
        report.registered,
        report.provider,
        guest_os_name(report.guest_os),
        architecture_name(report.guest_arch)
    )?;
    writeln!(
        output,
        "  expected_format={} observed_format={} size={} virtual_size={} sha256={}",
        report.expected_format,
        observed_format,
        report
            .file_size
            .map_or_else(|| "unknown".to_owned(), |size| size.to_string()),
        report
            .virtual_size
            .map_or_else(|| "unknown".to_owned(), |size| size.to_string()),
        sha256
    )?;
    writeln!(
        output,
        "  backing_parent={} issues={} path={}",
        parent,
        issues,
        report.path.display()
    )?;
    Ok(())
}

fn write_doctor_report(report: &DoctorReport, output: &mut impl Write) -> std::io::Result<()> {
    writeln!(
        output,
        "vmcell doctor status={} host={}/{} state_root={}",
        doctor_status_name(report.status),
        report.host_os,
        report.host_arch,
        report.state_root.display()
    )?;
    for provider in &report.providers {
        write_provider_probe(provider, output)?;
    }
    if report.providers.iter().any(|provider| provider.available) {
        writeln!(output, "next_action=none provider_probe=ready")
    } else {
        writeln!(
            output,
            "next_action=investigate_provider provider_probe=unavailable"
        )
    }
}

fn write_status_report(report: &StatusReport, output: &mut impl Write) -> std::io::Result<()> {
    writeln!(
        output,
        "vmcell status evaluated_at={} providers={} cells={} images={} operations={}",
        report.evaluated_at.to_rfc3339(),
        report.providers.len(),
        report.cells.len(),
        report.images.len(),
        report.operations.len()
    )?;
    for provider in &report.providers {
        write_provider_probe(provider, output)?;
    }
    for entry in &report.cells {
        match &entry.observation {
            StatusCellObservation::Observed { inspection } => {
                let operations = report
                    .operations
                    .iter()
                    .filter(|operation| operation.operation.cell_id == entry.cell.id)
                    .map(|operation| operation.operation.clone())
                    .collect::<Vec<_>>();
                write_cell_inspection(inspection, &operations, report.evaluated_at, output)?;
            }
            StatusCellObservation::ProviderUnavailable { provider_status } => {
                write_cell_summary(&entry.cell, report.evaluated_at, output)?;
                writeln!(
                    output,
                    "  observation=provider_unavailable provider_status={} required_action=manual_review",
                    provider_probe_status_name(*provider_status)
                )?;
            }
            StatusCellObservation::InspectionFailed { error_code } => {
                write_cell_summary(&entry.cell, report.evaluated_at, output)?;
                writeln!(
                    output,
                    "  observation=inspection_failed error_code={} required_action=manual_review",
                    error_code
                )?;
            }
        }
        writeln!(
            output,
            "  retention={} pending_operations={} uncertain_operations={} cleanup={}",
            status_retention_name(entry.retention),
            entry.pending_operations,
            entry.uncertain_operations,
            status_cleanup_name(entry.cleanup)
        )?;
    }
    for entry in &report.images {
        write_registered_image(&entry.image, output)?;
        if entry.observations.is_empty() {
            writeln!(
                output,
                "  validation=failed error_code=vmcell.image.integrity action=manual_review"
            )?;
        }
        for variant in &entry.observations {
            match &variant.observation {
                StatusImageObservation::Validated { report } => {
                    write_image_validation(report, output)?
                }
                StatusImageObservation::ProviderUnavailable { provider_status } => writeln!(
                    output,
                    "  validation=provider_unavailable provider={} provider_status={} action=investigate_provider",
                    variant.provider,
                    provider_probe_status_name(*provider_status)
                )?,
                StatusImageObservation::ValidationFailed { error_code } => writeln!(
                    output,
                    "  validation=failed provider={} error_code={} action=manual_review",
                    variant.provider, error_code
                )?,
            }
        }
    }
    for entry in &report.operations {
        write_guest_operation(&entry.operation, output)?;
    }
    Ok(())
}

fn write_provider_probe(probe: &ProviderProbe, output: &mut impl Write) -> std::io::Result<()> {
    let accelerators = joined_or_none(&probe.capabilities.accelerators);
    let transports = joined_or_none(&probe.capabilities.guest_transports);
    writeln!(
        output,
        "provider={} status={} available={} accelerators={} guest_transports={} action={} detail={}",
        probe.name,
        provider_probe_status_name(probe.status),
        probe.available,
        accelerators,
        transports,
        provider_probe_action(probe.status),
        probe.detail
    )
}

fn write_cell_summary(
    cell: &CellRecord,
    now: chrono::DateTime<chrono::Utc>,
    output: &mut impl Write,
) -> std::io::Result<()> {
    writeln!(
        output,
        "cell={} provider={} image={} state={} phase={} retention={} last_error={}",
        cell.id,
        cell.provider,
        cell.image.image_id,
        cell_state_name(cell.state),
        cell_phase_name(cell.phase),
        cell_retention(cell, now),
        cell.last_error.as_deref().unwrap_or("none")
    )
}

fn write_cell_inspection(
    inspection: &CellInspection,
    operations: &[GuestOperationRecord],
    evaluated_at: chrono::DateTime<chrono::Utc>,
    output: &mut impl Write,
) -> std::io::Result<()> {
    write_cell_summary(&inspection.cell, evaluated_at, output)?;
    let provider_power = inspection
        .provider_vm
        .as_ref()
        .map_or("absent", |provider| {
            provider_power_state_name(&provider.power_state)
        });
    writeln!(
        output,
        "  reconciliation={} ownership={} required_action={} cleanup={} provider_power={}",
        reconciliation_name(&inspection.reconciliation),
        ownership_name(inspection.classification.ownership),
        required_action_name(inspection.classification.required_action),
        cleanup_guidance(inspection, operations),
        provider_power
    )?;
    if let Some(detail) = reconciliation_detail(&inspection.reconciliation) {
        writeln!(output, "  detail={detail}")?;
    }
    if !operations.is_empty() {
        let pending = operations
            .iter()
            .filter(|operation| !operation.phase.is_terminal())
            .count();
        let uncertain = operations
            .iter()
            .filter(|operation| operation.phase == GuestOperationPhase::TransportActive)
            .count();
        let action = if uncertain > 0 {
            "manual_review"
        } else if pending > 0 {
            "reconcile_operation"
        } else {
            "none"
        };
        writeln!(
            output,
            "  guest_operations={} pending={} uncertain={} action={}",
            operations.len(),
            pending,
            uncertain,
            action
        )?;
    }
    Ok(())
}

fn write_guest_operation(
    operation: &GuestOperationRecord,
    output: &mut impl Write,
) -> std::io::Result<()> {
    writeln!(
        output,
        "operation={} cell={} kind={} phase={} failure={} action={} updated_at={}",
        operation.id,
        operation.cell_id,
        guest_operation_kind_name(operation.kind),
        guest_operation_phase_name(operation.phase),
        operation.failure.map_or("none", guest_failure_class_name),
        guest_operation_action(operation.phase),
        operation.updated_at.to_rfc3339()
    )
}

fn write_guest_operation_recovery(
    report: &GuestOperationRecoveryReport,
    output: &mut impl Write,
) -> std::io::Result<()> {
    write_guest_operation(&report.operation, output)?;
    writeln!(
        output,
        "  reconciliation={} changed={} required_action={}",
        guest_operation_recovery_name(report.disposition),
        report.changed,
        required_action_name(report.required_action)
    )
}

fn cell_retention(cell: &CellRecord, now: chrono::DateTime<chrono::Utc>) -> String {
    if cell.state == CellState::Destroyed {
        return "none".to_owned();
    }
    match cell.expires_at {
        Some(expires_at) if expires_at <= now => {
            format!("expired_since:{}", expires_at.to_rfc3339())
        }
        Some(expires_at) => format!("until:{}", expires_at.to_rfc3339()),
        None => "manual".to_owned(),
    }
}

fn cleanup_guidance(
    inspection: &CellInspection,
    operations: &[GuestOperationRecord],
) -> &'static str {
    if operations
        .iter()
        .any(|operation| !operation.phase.is_terminal())
    {
        return "refused_nonterminal_operation";
    }
    match inspection.classification.required_action {
        RequiredAction::None if inspection.cell.state == CellState::Destroyed => "not_needed",
        RequiredAction::None => "exact_owned_destroy",
        RequiredAction::RetryLifecycle => "reconcile_then_retry",
        RequiredAction::RecoveryRequired => "phase_recovery",
        RequiredAction::ManualReview => "refused_manual_review",
    }
}

fn reconciliation_name(status: &vm_cell_manager::engine::ReconciliationStatus) -> &'static str {
    use vm_cell_manager::engine::ReconciliationStatus;
    match status {
        ReconciliationStatus::ExactOwned => "exact_owned",
        ReconciliationStatus::ManifestOnly => "manifest_only",
        ReconciliationStatus::ProviderMissing => "provider_missing",
        ReconciliationStatus::UnprovenProviderObject { .. } => "unproven_provider_object",
        ReconciliationStatus::OwnershipMismatch { .. } => "ownership_mismatch",
        ReconciliationStatus::StateDrift { .. } => "state_drift",
        ReconciliationStatus::Provisioning { .. } => "provisioning",
        ReconciliationStatus::Destroyed => "destroyed",
    }
}

fn reconciliation_detail(status: &vm_cell_manager::engine::ReconciliationStatus) -> Option<String> {
    use vm_cell_manager::engine::ReconciliationStatus;
    match status {
        ReconciliationStatus::UnprovenProviderObject { id } => {
            Some(format!("provider_object_id={id}"))
        }
        ReconciliationStatus::OwnershipMismatch { reasons } => {
            Some(format!("ownership_mismatch={}", reasons.join(",")))
        }
        ReconciliationStatus::StateDrift {
            manifest_state,
            provider_state,
        } => Some(format!(
            "manifest_state={} provider_state={}",
            cell_state_name(*manifest_state),
            provider_power_state_name(provider_state)
        )),
        ReconciliationStatus::Provisioning { phase } => {
            Some(format!("provisioning_phase={}", cell_phase_name(*phase)))
        }
        _ => None,
    }
}

fn doctor_status_name(status: vm_cell_manager::cli::DoctorStatus) -> &'static str {
    match status {
        vm_cell_manager::cli::DoctorStatus::Ready => "ready",
        vm_cell_manager::cli::DoctorStatus::Unavailable => "unavailable",
    }
}

fn provider_probe_status_name(status: ProviderProbeStatus) -> &'static str {
    match status {
        ProviderProbeStatus::Ready => "ready",
        ProviderProbeStatus::UnsupportedHost => "unsupported_host",
        ProviderProbeStatus::Unavailable => "unavailable",
        ProviderProbeStatus::ProbeFailed => "probe_failed",
    }
}

fn provider_probe_action(status: ProviderProbeStatus) -> &'static str {
    match status {
        ProviderProbeStatus::Ready => "none",
        ProviderProbeStatus::UnsupportedHost => "use_supported_host",
        ProviderProbeStatus::Unavailable => "install_or_enable_provider",
        ProviderProbeStatus::ProbeFailed => "investigate_provider_probe",
    }
}

fn joined_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(",")
    }
}

fn status_retention_name(value: StatusRetention) -> &'static str {
    match value {
        StatusRetention::Manual => "manual",
        StatusRetention::ActiveUntilExpiry => "active_until_expiry",
        StatusRetention::Expired => "expired",
        StatusRetention::None => "none",
    }
}

fn status_cleanup_name(value: StatusCleanupGuidance) -> &'static str {
    match value {
        StatusCleanupGuidance::ExactOwnedDestroy => "exact_owned_destroy",
        StatusCleanupGuidance::PhaseRecovery => "phase_recovery",
        StatusCleanupGuidance::ReconcileThenRetry => "reconcile_then_retry",
        StatusCleanupGuidance::NotNeeded => "not_needed",
        StatusCleanupGuidance::ManualReview => "manual_review",
    }
}

fn cell_state_name(state: CellState) -> &'static str {
    match state {
        CellState::Creating => "creating",
        CellState::Stopped => "stopped",
        CellState::Running => "running",
        CellState::Destroying => "destroying",
        CellState::Destroyed => "destroyed",
        CellState::Failed => "failed",
    }
}

fn cell_phase_name(phase: CellPhase) -> &'static str {
    match phase {
        CellPhase::IntentRecorded => "intent_recorded",
        CellPhase::OverlayCreated => "overlay_created",
        CellPhase::ProviderObjectCreated => "provider_object_created",
        CellPhase::ProviderObjectClaimed => "provider_object_claimed",
        CellPhase::Ready => "ready",
        CellPhase::Destroying => "destroying",
        CellPhase::DestroyingProvisioning => "destroying_provisioning",
        CellPhase::Destroyed => "destroyed",
    }
}

fn ownership_name(
    value: vm_cell_manager::core::automation::OwnershipClassification,
) -> &'static str {
    use vm_cell_manager::core::automation::OwnershipClassification;
    match value {
        OwnershipClassification::Proven => "proven",
        OwnershipClassification::PhaseProven => "phase_proven",
        OwnershipClassification::Unproven => "unproven",
        OwnershipClassification::Mismatch => "mismatch",
        OwnershipClassification::NotApplicable => "not_applicable",
    }
}

fn required_action_name(value: RequiredAction) -> &'static str {
    match value {
        RequiredAction::None => "none",
        RequiredAction::RetryLifecycle => "retry_lifecycle",
        RequiredAction::RecoveryRequired => "recovery_required",
        RequiredAction::ManualReview => "manual_review",
    }
}

fn provider_power_state_name(value: &ProviderPowerState) -> &str {
    match value {
        ProviderPowerState::Off => "off",
        ProviderPowerState::Running => "running",
        ProviderPowerState::Paused => "paused",
        ProviderPowerState::Saved => "saved",
        ProviderPowerState::Other(value) => value.as_str(),
    }
}

fn guest_operation_kind_name(value: GuestOperationKind) -> &'static str {
    match value {
        GuestOperationKind::Exec => "exec",
        GuestOperationKind::CopyIn => "copy_in",
        GuestOperationKind::CopyOut => "copy_out",
        GuestOperationKind::ArtifactCollect => "artifact_collect",
    }
}

fn guest_operation_phase_name(value: GuestOperationPhase) -> &'static str {
    match value {
        GuestOperationPhase::IntentRecorded => "intent_recorded",
        GuestOperationPhase::TransportActive => "transport_active",
        GuestOperationPhase::ArtifactCommitted => "artifact_committed",
        GuestOperationPhase::Completed => "completed",
        GuestOperationPhase::Failed => "failed",
    }
}

fn guest_failure_class_name(value: GuestFailureClass) -> &'static str {
    match value {
        GuestFailureClass::Interrupted => "interrupted",
        GuestFailureClass::GuestNotReady => "guest_not_ready",
        GuestFailureClass::Authentication => "authentication",
        GuestFailureClass::Session => "session",
        GuestFailureClass::Timeout => "timeout",
        GuestFailureClass::OutputLimit => "output_limit",
        GuestFailureClass::InvalidEncoding => "invalid_encoding",
        GuestFailureClass::PathViolation => "path_violation",
        GuestFailureClass::PartialCopy => "partial_copy",
        GuestFailureClass::OwnershipChanged => "ownership_changed",
        GuestFailureClass::Unknown => "unknown",
    }
}

fn guest_operation_action(value: GuestOperationPhase) -> &'static str {
    match value {
        GuestOperationPhase::IntentRecorded | GuestOperationPhase::ArtifactCommitted => {
            "reconcile_operation"
        }
        GuestOperationPhase::TransportActive => "manual_review",
        GuestOperationPhase::Completed | GuestOperationPhase::Failed => "none",
    }
}

fn guest_operation_recovery_name(
    value: vm_cell_manager::engine::GuestOperationRecoveryDisposition,
) -> &'static str {
    use vm_cell_manager::engine::GuestOperationRecoveryDisposition;
    match value {
        GuestOperationRecoveryDisposition::AlreadyTerminal => "already_terminal",
        GuestOperationRecoveryDisposition::InterruptedBeforeTransport => {
            "interrupted_before_transport"
        }
        GuestOperationRecoveryDisposition::ArtifactCompletionRecovered => {
            "artifact_completion_recovered"
        }
        GuestOperationRecoveryDisposition::RecoveryRequired => "recovery_required",
    }
}

const fn guest_os_name(guest_os: GuestOs) -> &'static str {
    match guest_os {
        GuestOs::Windows => "windows",
        GuestOs::Linux => "linux",
        GuestOs::Macos => "macos",
    }
}

const fn architecture_name(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64",
    }
}

fn run_cell_guest<P: LocalVmProvider>(
    engine: &CellEngine<P>,
    credential: CredentialArgs,
    request: RunCellRequest,
    observer: &mut impl RunObserver,
) -> Result<RunCellReport, Box<dyn Error>> {
    let credentials = read_credentials(engine.provider_name(), credential)?;
    Ok(match engine.provider_name() {
        "hyperv" => engine.run_cell_observed(
            &PowerShellDirectTransport::system(),
            &credentials,
            request,
            observer,
        )?,
        "qemu" => engine.run_cell_observed(
            &QemuGuestAgentTransport::system(),
            &credentials,
            request,
            observer,
        )?,
        value => {
            return Err(
                EngineError::Integrity(format!("unsupported guest provider: {value}")).into(),
            );
        }
    })
}

fn guest_exit_status(exit_code: i32) -> ExitCode {
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(exit_code).unwrap_or(1))
    }
}

fn write_human_run_result(
    report: &RunCellReport,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> std::io::Result<()> {
    stdout.write_all(report.result.stdout.as_bytes())?;
    stderr.write_all(report.result.stderr.as_bytes())?;
    if !report.result.stderr.is_empty() && !report.result.stderr.ends_with('\n') {
        stderr.write_all(b"\n")?;
    }
    writeln!(
        stderr,
        "vmcell: run cell {}: exit={} cleanup={}",
        report.cell_id,
        report.result.exit_code,
        report.cleanup.as_str()
    )
}

struct HumanRunObserver<W: Write> {
    writer: W,
    error: Option<std::io::Error>,
}

impl<W: Write> HumanRunObserver<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            error: None,
        }
    }

    fn line(&mut self, arguments: std::fmt::Arguments<'_>) {
        if self.error.is_none() {
            if let Err(error) = writeln!(self.writer, "vmcell: {arguments}") {
                self.error = Some(error);
            }
        }
    }

    fn finish(self) -> std::io::Result<W> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(self.writer),
        }
    }
}

impl<W: Write> RunObserver for HumanRunObserver<W> {
    fn observe(&mut self, event: &RunProgressEvent) -> RunControl {
        match event {
            RunProgressEvent::ImageVerified { image } => {
                self.line(format_args!("image verified: {image}"));
            }
            RunProgressEvent::CellCreated { cell_id } => {
                self.line(format_args!("cell created: {cell_id}"));
            }
            RunProgressEvent::ProviderStarted { cell_id } => {
                self.line(format_args!("provider started: {cell_id}"));
            }
            RunProgressEvent::GuestReady { cell_id } => {
                self.line(format_args!("guest ready: {cell_id}"));
            }
            RunProgressEvent::CommandCompleted { cell_id, exit_code } => {
                self.line(format_args!(
                    "command completed: cell={cell_id} exit={exit_code}"
                ));
            }
            RunProgressEvent::CleanupStarted { cell_id } => {
                self.line(format_args!("cleanup started: {cell_id}"));
            }
            RunProgressEvent::CellDestroyed { cell_id } => {
                self.line(format_args!("cell destroyed: {cell_id}"));
            }
            RunProgressEvent::CellRetained {
                cell_id,
                disposition,
            } => {
                self.line(format_args!(
                    "cell retained: cell={cell_id} cleanup={}",
                    disposition.as_str()
                ));
            }
            RunProgressEvent::CleanupRefused { cell_id } => {
                self.line(format_args!(
                    "cleanup refused: cell={cell_id} reason=ambiguous_state"
                ));
            }
        }
        RunControl::Continue
    }
}

fn copy_in_guest<P: LocalVmProvider>(
    engine: &CellEngine<P>,
    credential: CredentialArgs,
    request: GuestCopyInRequest,
) -> Result<vm_cell_manager::engine::GuestCopyInReport, Box<dyn Error>> {
    let credentials = read_credentials(engine.provider_name(), credential)?;
    Ok(match engine.provider_name() {
        "hyperv" => {
            engine.copy_into_guest(&PowerShellDirectTransport::system(), &credentials, request)?
        }
        "qemu" => {
            engine.copy_into_guest(&QemuGuestAgentTransport::system(), &credentials, request)?
        }
        value => {
            return Err(
                EngineError::Integrity(format!("unsupported guest provider: {value}")).into(),
            );
        }
    })
}

fn copy_out_guest<P: LocalVmProvider>(
    engine: &CellEngine<P>,
    credential: CredentialArgs,
    request: GuestCopyOutRequest,
) -> Result<vm_cell_manager::engine::ArtifactReport, Box<dyn Error>> {
    let credentials = read_credentials(engine.provider_name(), credential)?;
    Ok(match engine.provider_name() {
        "hyperv" => {
            engine.copy_out_of_guest(&PowerShellDirectTransport::system(), &credentials, request)?
        }
        "qemu" => {
            engine.copy_out_of_guest(&QemuGuestAgentTransport::system(), &credentials, request)?
        }
        value => {
            return Err(
                EngineError::Integrity(format!("unsupported guest provider: {value}")).into(),
            );
        }
    })
}

fn collect_guest_artifacts<P: LocalVmProvider>(
    engine: &CellEngine<P>,
    credential: CredentialArgs,
    request: ArtifactCollectRequest,
) -> Result<vm_cell_manager::engine::ArtifactReport, Box<dyn Error>> {
    let credentials = read_credentials(engine.provider_name(), credential)?;
    Ok(match engine.provider_name() {
        "hyperv" => {
            engine.collect_artifacts(&PowerShellDirectTransport::system(), &credentials, request)?
        }
        "qemu" => {
            engine.collect_artifacts(&QemuGuestAgentTransport::system(), &credentials, request)?
        }
        value => {
            return Err(
                EngineError::Integrity(format!("unsupported guest provider: {value}")).into(),
            );
        }
    })
}

fn readiness(timeout_seconds: u64) -> ReadinessPolicy {
    ReadinessPolicy {
        timeout: Duration::from_secs(timeout_seconds),
        poll_interval: Duration::from_secs(2),
    }
}

fn read_credentials(
    provider: &str,
    args: CredentialArgs,
) -> Result<GuestCredentials, Box<dyn Error>> {
    if provider == "qemu" {
        if args.username.is_some() || args.password_stdin {
            return Err(CliInputError(
                "QGA is credentialless; do not pass username or password flags".to_owned(),
            )
            .into());
        }
        return Ok(GuestCredentials::not_required());
    }
    if !args.password_stdin {
        return Err(CliInputError(
            "guest password must be provided with --password-stdin".to_owned(),
        )
        .into());
    }
    let username = args
        .username
        .ok_or_else(|| CliInputError("PowerShell Direct requires --username".to_owned()))?;
    let mut password = Zeroizing::new(String::new());
    if let Err(error) = std::io::stdin().take(4097).read_to_string(&mut password) {
        if error.kind() == std::io::ErrorKind::InvalidData {
            return Err(
                CliInputError("guest password stdin must be valid UTF-8".to_owned()).into(),
            );
        }
        return Err(error.into());
    }
    while password.ends_with(['\r', '\n']) {
        password.pop();
    }
    if password.len() > 4096 || password.contains(['\r', '\n']) {
        return Err(
            CliInputError("guest password stdin must contain one bounded line".to_owned()).into(),
        );
    }
    Ok(GuestCredentials::new(
        username,
        std::mem::take(&mut *password),
    )?)
}

fn emit<T: Serialize>(
    value: &T,
    json: bool,
    human: impl FnOnce(),
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        human();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cell(
        state: CellState,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> CellRecord {
        let cell_id = vm_cell_manager::core::cell::CellId::new();
        let image_id: vm_cell_manager::core::image::ImageId = "windows-dev".parse().unwrap();
        let ownership = vm_cell_manager::core::ownership::CellOwnership::new(
            uuid::Uuid::nil(),
            cell_id,
            uuid::Uuid::from_u128(2),
            std::path::PathBuf::from(r"C:\state\cell\hyperv"),
            std::path::PathBuf::from(r"C:\state\cell\cell.vhdx"),
        );
        CellRecord {
            schema_version: 1,
            id: cell_id,
            provider: "hyperv".to_owned(),
            spec: CellSpec {
                image: image_id.clone(),
                provider: Some("hyperv".to_owned()),
                cpu_count: 2,
                memory_mib: 4096,
                ttl_seconds: None,
                accelerator: None,
                allow_tcg: false,
            },
            image: vm_cell_manager::core::image::ImageBinding {
                image_id,
                guest_os: Some(GuestOs::Windows),
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: std::path::PathBuf::from(r"C:\images\base.vhdx"),
                sha256: "a".repeat(64),
                file_size: 1024,
            },
            ownership,
            provider_object: None,
            state,
            phase: if state == CellState::Destroyed {
                CellPhase::Destroyed
            } else {
                CellPhase::Ready
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            expires_at,
            last_error: None,
        }
    }

    #[test]
    fn run_guest_exit_status_is_propagated_when_representable() {
        assert_eq!(guest_exit_status(0), ExitCode::SUCCESS);
        assert_eq!(guest_exit_status(23), ExitCode::from(23));
        assert_eq!(guest_exit_status(-1), ExitCode::from(1));
        assert_eq!(guest_exit_status(256), ExitCode::from(1));
    }

    #[test]
    fn human_run_result_forwards_bounded_guest_streams_and_separates_status() {
        let report = RunCellReport {
            schema_version: 1,
            cell_id: vm_cell_manager::core::cell::CellId::new(),
            operation_id: vm_cell_manager::core::guest::GuestOperationId::new(),
            outcome: vm_cell_manager::engine::RunOutcome::GuestNonZero,
            result: vm_cell_manager::guest::GuestCommandResult {
                exit_code: 23,
                stdout: "guest stdout\n".to_owned(),
                stderr: "guest stderr".to_owned(),
                encoding: "utf-8".to_owned(),
                stdout_bytes: 13,
                stderr_bytes: 12,
                truncated: false,
            },
            cleanup: vm_cell_manager::engine::RunCleanupDisposition::Destroyed,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        write_human_run_result(&report, &mut stdout, &mut stderr).unwrap();

        assert_eq!(stdout, b"guest stdout\n");
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.starts_with("guest stderr\nvmcell:"));
        assert!(stderr.contains("exit=23"));
        assert!(stderr.contains("cleanup=destroyed"));
    }

    #[test]
    fn human_run_observer_reports_safe_lifecycle_progress() {
        let image = "windows-dev".parse().unwrap();
        let cell_id = vm_cell_manager::core::cell::CellId::new();
        let mut observer = HumanRunObserver::new(Vec::new());
        for event in [
            RunProgressEvent::ImageVerified { image },
            RunProgressEvent::CellCreated { cell_id },
            RunProgressEvent::ProviderStarted { cell_id },
            RunProgressEvent::GuestReady { cell_id },
            RunProgressEvent::CommandCompleted {
                cell_id,
                exit_code: 0,
            },
            RunProgressEvent::CleanupStarted { cell_id },
            RunProgressEvent::CellDestroyed { cell_id },
            RunProgressEvent::CellRetained {
                cell_id,
                disposition: vm_cell_manager::engine::RunCleanupDisposition::RetainedByRequest,
            },
            RunProgressEvent::CleanupRefused { cell_id },
        ] {
            assert_eq!(observer.observe(&event), RunControl::Continue);
        }
        let output = String::from_utf8(observer.finish().unwrap()).unwrap();

        assert!(output.contains("vmcell: image verified: windows-dev"));
        assert!(output.contains(&format!("vmcell: cell created: {cell_id}")));
        assert!(output.contains(&format!("vmcell: provider started: {cell_id}")));
        assert!(output.contains(&format!("vmcell: guest ready: {cell_id}")));
        assert!(output.contains(&format!("vmcell: command completed: cell={cell_id} exit=0")));
        assert!(output.contains(&format!("vmcell: cleanup started: {cell_id}")));
        assert!(output.contains(&format!("vmcell: cell destroyed: {cell_id}")));
        assert!(output.contains(&format!(
            "vmcell: cell retained: cell={cell_id} cleanup=retained_by_request"
        )));
        assert!(output.contains(&format!(
            "vmcell: cleanup refused: cell={cell_id} reason=ambiguous_state"
        )));
        assert!(!output.contains("credential-sentinel"));
    }

    #[test]
    fn human_image_validation_reports_identity_hash_backing_and_issues() {
        let report = ImageValidationReport {
            schema_version: 1,
            image_id: Some("windows-dev".parse().unwrap()),
            registered: true,
            provider: "hyperv".to_owned(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            path: std::path::PathBuf::from(r"C:\images\base.vhdx"),
            expected_format: "vhdx".to_owned(),
            observed_format: Some("vhdx".to_owned()),
            disk_type: Some("fixed".to_owned()),
            parent_path: None,
            file_size: Some(1024),
            virtual_size: Some(4096),
            sha256: Some("a".repeat(64)),
            registered_sha256: Some("b".repeat(64)),
            status: ImageValidationStatus::Unusable,
            issues: vec![vm_cell_manager::engine::ImageValidationIssue::RegisteredHashDrift],
        };
        let mut output = Vec::new();

        write_image_validation(&report, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("validation=unusable image=windows-dev registered=true"));
        assert!(output.contains("provider=hyperv guest_os=windows guest_arch=x86_64"));
        assert!(output.contains(&"a".repeat(64)));
        assert!(output.contains("backing_parent=none issues=registered_hash_drift"));
        assert!(!output.contains(&"b".repeat(64)));
    }

    #[test]
    fn human_status_derives_retention_and_never_calls_expired_safe_without_proof() {
        let evaluated_at = chrono::Utc::now();
        let expired = test_cell(
            CellState::Running,
            Some(evaluated_at - chrono::Duration::seconds(1)),
        );
        let retained = test_cell(CellState::Stopped, None);
        let destroyed = test_cell(CellState::Destroyed, None);

        assert_eq!(
            status_retention(&expired, evaluated_at),
            StatusRetention::Expired
        );
        assert_eq!(
            status_retention(&retained, evaluated_at),
            StatusRetention::Manual
        );
        assert_eq!(
            status_retention(&destroyed, evaluated_at),
            StatusRetention::None
        );
    }

    #[test]
    fn human_operation_status_marks_transport_active_as_uncertain_manual_review() {
        let cell_id = vm_cell_manager::core::cell::CellId::new();
        let mut operation =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::Exec, chrono::Utc::now());
        operation.phase = GuestOperationPhase::TransportActive;
        let mut output = Vec::new();

        write_guest_operation(&operation, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("phase=transport_active"));
        assert!(output.contains("action=manual_review"));
        assert_eq!(
            guest_operation_required_action(operation.phase),
            RequiredAction::ManualReview
        );

        let inspection = CellInspection {
            schema_version: 1,
            cell: test_cell(CellState::Running, None),
            provider_vm: None,
            classification: vm_cell_manager::engine::ReconciliationClassification {
                code: vm_cell_manager::engine::ReconciliationCode::ExactOwned,
                ownership: vm_cell_manager::core::automation::OwnershipClassification::Proven,
                required_action: RequiredAction::None,
            },
            reconciliation: vm_cell_manager::engine::ReconciliationStatus::ExactOwned,
        };
        assert_eq!(
            cleanup_guidance(&inspection, &[operation]),
            "refused_nonterminal_operation"
        );
    }

    #[test]
    fn doctor_human_output_explains_typed_unavailability_without_claiming_admission() {
        let report = DoctorReport {
            schema_version: 1,
            contract: vm_cell_manager::core::automation::DOCTOR_CONTRACT,
            status: vm_cell_manager::cli::DoctorStatus::Unavailable,
            host_os: "windows",
            host_arch: "x86_64",
            state_root: std::path::PathBuf::from(r"C:\state"),
            providers: vec![ProviderProbe {
                name: "hyperv",
                status: ProviderProbeStatus::Unavailable,
                available: false,
                detail: "provider unavailable".to_owned(),
                capabilities: vm_cell_manager::core::capability::ProviderCapabilities::unavailable(
                ),
            }],
        };
        let mut output = Vec::new();

        write_doctor_report(&report, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("status=unavailable"));
        assert!(output.contains("action=install_or_enable_provider"));
        assert!(output.contains("next_action=investigate_provider"));
        assert!(!output.contains("admitted"));
    }
}
