use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::ExitCode;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{CommandFactory, Parser, error::ErrorKind};
use clap_complete::{
    generate,
    shells::{Bash, PowerShell, Zsh},
};
use serde::Serialize;
use vm_cell_manager::cli::{
    ArtifactCommand, Cli, CliExitCode, CliHumanOutput, CliInputError, CliProvider, Command,
    CompletionCommand, CredentialArgs, DoctorReport, ErrorEnvelope, GuestOperationCommand,
    ImageCommand, JobCommand, ListEnvelope, ProviderCommand, ReceiptCommand, RunErrorEnvelope,
    StateCommand, StatusCellEntry, StatusCellObservation, StatusCleanupGuidance, StatusImageEntry,
    StatusImageObservation, StatusImageVariantObservation, StatusOperationEntry, StatusReport,
    StatusRetention, classify_cli_error, public_error_message,
};
use vm_cell_manager::config::{ConfigProvider, HumanOutputPreference, ResolvedConfig, load_config};
use vm_cell_manager::core::acceptance_receipt::{
    AcceptanceReceiptDisposition, AcceptanceReceiptValidationReport, MAX_ACCEPTANCE_RECEIPT_BYTES,
    validate_acceptance_receipt_bytes,
};
use vm_cell_manager::core::automation::RequiredAction;
use vm_cell_manager::core::cell::{CellPhase, CellRecord, CellSpec, CellState};
use vm_cell_manager::core::guest::{
    GuestFailureClass, GuestOperationKind, GuestOperationPhase, GuestOperationRecord,
};
use vm_cell_manager::core::image::{Architecture, GuestOs, ImageRecord};
use vm_cell_manager::core::job_plan::{ResolvedJobPlan, resolve_job_plan};
use vm_cell_manager::core::job_spec::load_job_spec;
use vm_cell_manager::core::run_selection::{
    HostPlatform, RequestedAccelerator, RunExecutionPlan, RunSelectionIntent, RunSelectionSource,
    resolve_run_execution_plan,
};
use vm_cell_manager::core::support::{Accelerator, ProviderId};
#[cfg(test)]
use vm_cell_manager::core::support::{GuestTransportId, HostOs, SupportStatus};
#[cfg(test)]
use vm_cell_manager::engine::run_request_validation_error;
use vm_cell_manager::engine::{
    ArtifactCollectRequest, ArtifactPruneRequest, CellEngine, CellInspection, EngineError,
    GuestCopyInRequest, GuestCopyOutRequest, GuestExecReport, GuestExecRequest,
    GuestOperationRecoveryReport, ImageDependencyReport, ImageUnregisterReport,
    ImageValidationReport, ImageValidationStatus, JobRunRequest, RegisterImageRequest,
    RunCellError, RunCellReport, RunCellRequest, RunCleanupPolicy, RunControl, RunObserver,
    RunProgressEvent, ValidateImageRequest, build_job_run_request, inspect_image_dependencies,
    run_request_validation_error_with_job, unregister_image, validate_run_resources,
};
use vm_cell_manager::guest::powershell_direct::PowerShellDirectTransport;
use vm_cell_manager::guest::qga::QemuGuestAgentTransport;
use vm_cell_manager::guest::{
    DEFAULT_MAX_OUTPUT_BYTES, GuestCommand, GuestCredentials, GuestIoError, ReadinessPolicy,
};
use vm_cell_manager::providers::hyperv::HyperVProvider;
use vm_cell_manager::providers::qemu::QemuProvider;
use vm_cell_manager::providers::{
    LocalVmProvider, ProviderPowerState, ProviderProbe, ProviderProbeStatus,
    builtin_provider_probes,
};
use vm_cell_manager::state::{StateCompatibilityReport, StateCompatibilityStatus, StateStore};
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
        let job = report
            .job
            .as_ref()
            .map_or_else(String::new, |job| format!(" job={}", job.job_id));
        eprintln!(
            "vmcell: {}: {message}; run stage={} cell={} operation={} cleanup={} cleanup_error={}{}",
            classification.code,
            report.stage.as_str(),
            cell,
            operation,
            report.cleanup.as_str(),
            cleanup_error,
            job,
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
    if matches!(&cli.command, Command::Shell { .. }) {
        require_human_shell(cli.json)?;
    }
    if let Command::Completion { command } = &cli.command {
        if cli.json {
            return Err(CliInputError(
                "completion output is a shell script and does not support --json".to_owned(),
            )
            .into());
        }
        write_completion(*command, &mut std::io::stdout().lock());
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(
        &cli.command,
        Command::Receipt {
            command: ReceiptCommand::Validate
        }
    ) {
        let stdin = std::io::stdin();
        let bytes = read_bounded_acceptance_receipt(&mut stdin.lock())?;
        let report = validate_acceptance_receipt_bytes(&bytes);
        emit(&report, cli.json, || {
            write_acceptance_receipt_validation(&report, &mut std::io::stdout().lock())
                .expect("stdout should remain writable");
        })?;
        return Ok(ExitCode::from(
            if report.disposition == AcceptanceReceiptDisposition::Pass {
                CliExitCode::Success
            } else {
                CliExitCode::Integrity
            }
            .as_u8(),
        ));
    }
    let defaults = load_config(cli.config.as_deref())?;
    let state_root = cli
        .state_root
        .clone()
        .or_else(|| defaults.state_root.clone());
    let lock_timeout =
        Duration::from_millis(cli.lock_timeout_ms.unwrap_or(defaults.lock_timeout_ms));
    let human_output = cli
        .human_output
        .map_or(defaults.human_output, |value| match value {
            CliHumanOutput::Normal => HumanOutputPreference::Normal,
            CliHumanOutput::Quiet => HumanOutputPreference::Quiet,
        });
    if let Command::Run {
        spec: None,
        cpu_count,
        memory_mib,
        ttl_seconds,
        ..
    } = &cli.command
    {
        validate_run_resources(
            cpu_count.unwrap_or(defaults.cpu_count),
            memory_mib.unwrap_or(defaults.memory_mib),
            *ttl_seconds,
        )?;
    }
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
        Command::State {
            command: StateCommand::Check,
        } => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let report = StateStore::new(root).check_compatibility()?;
            emit(&report, cli.json, || {
                write_state_compatibility(&report, &mut std::io::stdout().lock())
                    .expect("stdout should remain writable");
            })?;
        }
        Command::Completion { .. } => unreachable!("handled before configuration loading"),
        Command::Receipt { .. } => unreachable!("handled before configuration loading"),
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
        Command::Job {
            command: JobCommand::Plan { spec },
        } => {
            let loaded = load_job_spec(&spec)?;
            let state = StateStore::new(state_root.unwrap_or_else(StateStore::default_root));
            let image = state.load_image(&loaded.spec().image)?;
            let plan = resolve_job_plan(
                &loaded,
                HostPlatform::current()?,
                &image,
                &builtin_provider_probes(),
            )?;
            emit(&plan, cli.json, || {
                write_human_job_plan(&plan, &mut std::io::stdout().lock())
                    .expect("stdout should remain writable");
            })?;
        }
        Command::Run {
            spec: Some(spec),
            plan_only,
            credential,
            ..
        } => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let state = StateStore::new(root.clone()).with_mutation_lock_timeout(lock_timeout);
            let loaded = load_job_spec(&spec)?;
            let image = state.load_image(&loaded.spec().image)?;
            let host = HostPlatform::current()?;
            let probes = builtin_provider_probes();
            let plan = resolve_job_plan(&loaded, host, &image, &probes)?;
            if plan_only {
                emit(&plan, cli.json, || {
                    write_human_job_plan(&plan, &mut std::io::stdout().lock())
                        .expect("stdout should remain writable");
                })?;
                return Ok(ExitCode::SUCCESS);
            }
            let (plan, request) = build_job_run_request(&loaded, host, &image, &probes)?;
            if !cli.json && human_output == HumanOutputPreference::Normal {
                write_human_job_plan(&plan, &mut std::io::stderr().lock())?;
            }
            let interrupt = install_job_run_interrupt(request.plan(), request.job())?;
            let report = match plan.execution.provider {
                ProviderId::Hyperv => {
                    let engine = CellEngine::new(state, HyperVProvider::system());
                    if cli.json || human_output == HumanOutputPreference::Quiet {
                        let mut observer = |_event: &RunProgressEvent| {
                            if interrupt.requested() {
                                RunControl::Cancel
                            } else {
                                RunControl::Continue
                            }
                        };
                        run_job_cell_guest(&engine, credential, request, &mut observer)?
                    } else {
                        let mut observer =
                            HumanRunObserver::with_interrupt(std::io::stderr(), || {
                                interrupt.requested()
                            });
                        let result =
                            run_job_cell_guest(&engine, credential, request, &mut observer);
                        let output_result = observer.finish().map(|_| ());
                        let report = result?;
                        output_result?;
                        report
                    }
                }
                ProviderId::Qemu => {
                    let engine = CellEngine::new(state, QemuProvider::system(root));
                    if cli.json || human_output == HumanOutputPreference::Quiet {
                        let mut observer = |_event: &RunProgressEvent| {
                            if interrupt.requested() {
                                RunControl::Cancel
                            } else {
                                RunControl::Continue
                            }
                        };
                        run_job_cell_guest(&engine, credential, request, &mut observer)?
                    } else {
                        let mut observer =
                            HumanRunObserver::with_interrupt(std::io::stderr(), || {
                                interrupt.requested()
                            });
                        let result =
                            run_job_cell_guest(&engine, credential, request, &mut observer);
                        let output_result = observer.finish().map(|_| ());
                        let report = result?;
                        output_result?;
                        report
                    }
                }
            };
            if cli.json {
                emit(&report, true, || {})?;
            } else {
                let stdout = std::io::stdout();
                let stderr = std::io::stderr();
                write_human_run_result(&report, &mut stdout.lock(), &mut stderr.lock())?;
            }
            return Ok(guest_exit_status(report.result.exit_code));
        }
        Command::Image {
            command: ImageCommand::Dependencies { id },
        } => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let state = StateStore::new(root).with_mutation_lock_timeout(lock_timeout);
            let report = inspect_image_dependencies(&state, &id)?;
            emit(&report, cli.json, || {
                write_image_dependencies(&report, &mut std::io::stdout().lock())
                    .expect("stdout should remain writable");
            })?;
        }
        Command::Image {
            command: ImageCommand::Unregister { id },
        } => {
            let root = state_root.unwrap_or_else(StateStore::default_root);
            let state = StateStore::new(root).with_mutation_lock_timeout(lock_timeout);
            let report = unregister_image(&state, &id)?;
            emit(&report, cli.json, || {
                write_image_unregister(&report, &mut std::io::stdout().lock())
                    .expect("stdout should remain writable");
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
            let run_plan = match &command {
                Command::Run {
                    image,
                    provider,
                    accelerator,
                    allow_tcg,
                    ..
                } => {
                    let image = state.load_image(image.as_ref().ok_or_else(|| {
                        CliInputError("--image is required unless --spec is supplied".to_owned())
                    })?)?;
                    Some(resolve_run_execution_plan(
                        HostPlatform::current()?,
                        &image,
                        &builtin_provider_probes(),
                        RunSelectionIntent {
                            explicit_provider: provider.map(provider_id_from_cli),
                            config_provider_preference: defaults
                                .provider_preference
                                .map(provider_id_from_config),
                            explicit_accelerator: accelerator.map(accelerator_request_from_cli),
                            allow_tcg: *allow_tcg,
                        },
                    )?)
                }
                _ => None,
            };
            let provider = run_plan.as_ref().map_or_else(
                || provider_for_command(&command, &state, defaults.provider),
                |plan| Ok(plan.provider.as_str().to_owned()),
            )?;
            return match provider.as_str() {
                "hyperv" => run_m2(
                    command,
                    cli.json,
                    &CellEngine::new(state, HyperVProvider::system()),
                    &defaults,
                    human_output,
                    run_plan,
                ),
                "qemu" => run_m2(
                    command,
                    cli.json,
                    &CellEngine::new(state, QemuProvider::system(root)),
                    &defaults,
                    human_output,
                    run_plan,
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

fn write_completion(command: CompletionCommand, output: &mut dyn Write) {
    let mut command_line = Cli::command();
    match command {
        CompletionCommand::Bash => {
            generate(Bash, &mut command_line, "vmcell", output);
        }
        CompletionCommand::Powershell => {
            generate(PowerShell, &mut command_line, "vmcell", output);
        }
        CompletionCommand::Zsh => {
            generate(Zsh, &mut command_line, "vmcell", output);
        }
    }
}

fn read_bounded_acceptance_receipt(input: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut bounded = input.take((MAX_ACCEPTANCE_RECEIPT_BYTES + 1) as u64);
    let mut bytes = Vec::with_capacity(MAX_ACCEPTANCE_RECEIPT_BYTES.min(8192));
    bounded.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_acceptance_receipt_validation(
    report: &AcceptanceReceiptValidationReport,
    output: &mut impl Write,
) -> std::io::Result<()> {
    let disposition = match report.disposition {
        AcceptanceReceiptDisposition::Pass => "pass",
        AcceptanceReceiptDisposition::PreflightOnly => "preflight_only",
        AcceptanceReceiptDisposition::TerminalNotPass => "terminal_not_pass",
        AcceptanceReceiptDisposition::Rejected => "rejected",
    };
    writeln!(
        output,
        "receipt validation: disposition={disposition} document_valid={} authorizing=false support_promotion=not_evaluated",
        report.document_valid
    )?;
    for finding in &report.findings {
        writeln!(output, "finding={}", finding.code)?;
    }
    Ok(())
}

fn run_m2<P: LocalVmProvider>(
    command: Command,
    json: bool,
    engine: &CellEngine<P>,
    defaults: &ResolvedConfig,
    human_output: HumanOutputPreference,
    run_plan: Option<RunExecutionPlan>,
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
            ImageCommand::Dependencies { .. } | ImageCommand::Unregister { .. } => {
                unreachable!("provider-neutral image command was routed before provider selection")
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
                cpu_count: cpu_count.unwrap_or(defaults.cpu_count),
                memory_mib: memory_mib.unwrap_or(defaults.memory_mib),
                ttl_seconds,
                accelerator: accelerator.map(|value| value.as_str().to_owned()),
                allow_tcg,
            })?;
            emit(&cell, json, || {
                println!("created cell {} ({:?})", cell.id, cell.state)
            })?;
        }
        Command::Run {
            spec: _,
            image,
            cpu_count,
            memory_mib,
            ttl_seconds,
            provider: _,
            accelerator: _,
            allow_tcg: _,
            plan_only,
            keep,
            keep_on_failure,
            credential,
            readiness_timeout_seconds,
            action_timeout_seconds,
            max_output_bytes,
            mut command,
        } => {
            let plan = run_plan.ok_or_else(|| {
                EngineError::Integrity(
                    "run command was routed without an execution plan".to_owned(),
                )
            })?;
            if plan_only {
                emit(&plan, json, || {
                    write_human_run_plan(&plan, &mut std::io::stdout().lock())
                        .expect("stdout should remain writable");
                })?;
                return Ok(ExitCode::SUCCESS);
            }
            if !json && human_output == HumanOutputPreference::Normal {
                write_human_run_plan(&plan, &mut std::io::stderr().lock())?;
            }
            let image = image.ok_or_else(|| {
                CliInputError("--image is required unless --spec is supplied".to_owned())
            })?;
            let program = command.remove(0);
            let request = RunCellRequest {
                plan: plan.clone(),
                spec: CellSpec {
                    image,
                    provider: Some(engine.provider_name().to_owned()),
                    cpu_count: cpu_count.unwrap_or(defaults.cpu_count),
                    memory_mib: memory_mib.unwrap_or(defaults.memory_mib),
                    ttl_seconds,
                    accelerator: (plan.provider == ProviderId::Qemu)
                        .then(|| plan.accelerator.as_str().to_owned()),
                    allow_tcg: plan.accelerator == Accelerator::Tcg,
                },
                command: GuestCommand {
                    program,
                    args: command,
                    timeout: Duration::from_secs(
                        action_timeout_seconds.unwrap_or(defaults.action_timeout_seconds),
                    ),
                    max_output_bytes: max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES),
                },
                readiness: readiness(
                    readiness_timeout_seconds.unwrap_or(defaults.readiness_timeout_seconds),
                ),
                cleanup: RunCleanupPolicy {
                    keep,
                    keep_on_failure,
                },
            };
            let interrupt = install_run_interrupt(&plan)?;
            let report = if json || human_output == HumanOutputPreference::Quiet {
                let mut observer = |_event: &RunProgressEvent| {
                    if interrupt.requested() {
                        RunControl::Cancel
                    } else {
                        RunControl::Continue
                    }
                };
                run_cell_guest(engine, credential, request, &mut observer)?
            } else {
                let mut observer =
                    HumanRunObserver::with_interrupt(std::io::stderr(), || interrupt.requested());
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
                        timeout: Duration::from_secs(
                            timeout_seconds.unwrap_or(defaults.action_timeout_seconds),
                        ),
                        max_output_bytes,
                    },
                    readiness: readiness(
                        readiness_timeout_seconds.unwrap_or(defaults.readiness_timeout_seconds),
                    ),
                },
            )?;
            emit(&report, json, || {
                println!("guest exec {}", report.operation_id)
            })?;
        }
        Command::Shell {
            cell_id,
            credential,
            readiness_timeout_seconds,
            action_timeout_seconds,
            max_output_bytes,
        } => {
            if engine.provider_name() != "hyperv" {
                return Err(CliInputError(
                    "vmcell shell currently supports Hyper-V Windows cells through PowerShell Direct"
                        .to_owned(),
                )
                .into());
            }
            let inspection = engine.inspect_cell(cell_id)?;
            if inspection.classification.code
                != vm_cell_manager::engine::ReconciliationCode::ExactOwned
                || inspection.cell.state != CellState::Running
                || inspection.cell.phase != CellPhase::Ready
                || inspection.cell.image.guest_os != Some(GuestOs::Windows)
            {
                return Err(EngineError::LifecycleConflict(
                    "shell requires an exact-owned, ready, running Windows cell".to_owned(),
                )
                .into());
            }
            if let Some(operation) = engine
                .list_guest_operations(Some(cell_id))?
                .into_iter()
                .find(|operation| !operation.phase.is_terminal())
            {
                let action = if operation.phase == GuestOperationPhase::TransportActive {
                    "manual_review"
                } else {
                    "operation_reconcile"
                };
                eprintln!(
                    "vmcell shell: nonterminal operation={} phase={} action={action}; session refused",
                    operation.id,
                    guest_operation_phase_name(operation.phase)
                );
                return Err(EngineError::LifecycleConflict(
                    "shell refuses a cell with a nonterminal guest operation".to_owned(),
                )
                .into());
            }
            let interrupt = ConsoleInterruptGuard::install()?;
            let console = open_windows_console_input()?;
            let credentials = read_credentials(engine.provider_name(), credential)?;
            let mut executor = EngineShellCommandExecutor {
                engine,
                credentials: &credentials,
                cell_id,
                readiness: readiness(
                    readiness_timeout_seconds.unwrap_or(defaults.readiness_timeout_seconds),
                ),
                action_timeout: Duration::from_secs(
                    action_timeout_seconds.unwrap_or(defaults.action_timeout_seconds),
                ),
                max_output_bytes,
            };
            let mut input = BufReader::new(console);
            let stdout = std::io::stdout();
            let stderr = std::io::stderr();
            let report = run_shell_session(
                &mut executor,
                &mut input,
                &mut stdout.lock(),
                &mut stderr.lock(),
                || interrupt.requested(),
            )?;
            return Ok(match report.end {
                ShellSessionEnd::Interrupted => ExitCode::from(130),
                ShellSessionEnd::Eof | ShellSessionEnd::ExitRequested => {
                    guest_exit_status(report.last_exit_code)
                }
            });
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
                    timeout: Duration::from_secs(
                        timeout_seconds.unwrap_or(defaults.action_timeout_seconds),
                    ),
                    max_bytes,
                    readiness: readiness(
                        readiness_timeout_seconds.unwrap_or(defaults.readiness_timeout_seconds),
                    ),
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
                    timeout: Duration::from_secs(
                        timeout_seconds.unwrap_or(defaults.action_timeout_seconds),
                    ),
                    max_bytes,
                    readiness: readiness(
                        readiness_timeout_seconds.unwrap_or(defaults.readiness_timeout_seconds),
                    ),
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
                        timeout: Duration::from_secs(
                            timeout_seconds.unwrap_or(defaults.action_timeout_seconds),
                        ),
                        max_bytes_per_file,
                        readiness: readiness(
                            readiness_timeout_seconds.unwrap_or(defaults.readiness_timeout_seconds),
                        ),
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
        Command::Doctor
        | Command::Status
        | Command::State { .. }
        | Command::Completion { .. }
        | Command::Receipt { .. }
        | Command::Provider { .. }
        | Command::Job { .. } => {
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

fn provider_for_command(
    command: &Command,
    state: &StateStore,
    configured_provider: ConfigProvider,
) -> Result<String, Box<dyn Error>> {
    let provider = match command {
        Command::Image {
            command: ImageCommand::Add { provider, .. },
        }
        | Command::Create { provider, .. }
        | Command::Run { provider, .. } => provider
            .map(CliProvider::as_str)
            .unwrap_or_else(|| configured_provider.as_str())
            .to_owned(),
        Command::Image {
            command: ImageCommand::Validate {
                id: None, provider, ..
            },
        } => provider
            .map(CliProvider::as_str)
            .unwrap_or_else(|| configured_provider.as_str())
            .to_owned(),
        Command::Image {
            command:
                ImageCommand::Validate {
                    id: Some(_),
                    provider: Some(provider),
                    ..
                },
        } => provider.as_str().to_owned(),
        Command::Image {
            command:
                ImageCommand::Validate {
                    id: Some(id),
                    provider: None,
                    ..
                },
        } => {
            let image = state.load_image(id)?;
            match image.variants.as_slice() {
                [variant] => variant.provider.clone(),
                [] => {
                    return Err(EngineError::ImageIntegrity(
                        "registered image does not contain a provider variant".to_owned(),
                    )
                    .into());
                }
                _ => {
                    return Err(CliInputError(
                        "--provider is required for a registered image with multiple variants"
                            .to_owned(),
                    )
                    .into());
                }
            }
        }
        Command::Inspect { cell_id }
        | Command::Start { cell_id }
        | Command::Stop { cell_id }
        | Command::Destroy { cell_id }
        | Command::Exec { cell_id, .. }
        | Command::Shell { cell_id, .. }
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
        | Command::State { .. }
        | Command::Completion { .. }
        | Command::Receipt { .. }
        | Command::Doctor
        | Command::Status
        | Command::Provider { .. } => CliProvider::Hyperv.as_str().to_owned(),
        Command::Job { .. } => {
            return Err("job plan command was not routed before provider selection".into());
        }
        Command::Reconcile { cell_id: None } | Command::Gc => {
            return Err("multi-provider command was not routed before provider selection".into());
        }
        Command::Image {
            command: ImageCommand::Dependencies { .. } | ImageCommand::Unregister { .. },
        } => {
            return Err(
                "provider-neutral image command was not routed before provider selection".into(),
            );
        }
    };
    Ok(provider)
}

const fn provider_id_from_cli(provider: CliProvider) -> ProviderId {
    match provider {
        CliProvider::Hyperv => ProviderId::Hyperv,
        CliProvider::Qemu => ProviderId::Qemu,
    }
}

const fn provider_id_from_config(provider: ConfigProvider) -> ProviderId {
    match provider {
        ConfigProvider::Hyperv => ProviderId::Hyperv,
        ConfigProvider::Qemu => ProviderId::Qemu,
    }
}

const fn accelerator_request_from_cli(
    accelerator: vm_cell_manager::cli::CliAccelerator,
) -> RequestedAccelerator {
    match accelerator {
        vm_cell_manager::cli::CliAccelerator::Auto => RequestedAccelerator::Auto,
        vm_cell_manager::cli::CliAccelerator::Whpx => RequestedAccelerator::Whpx,
        vm_cell_manager::cli::CliAccelerator::Kvm => RequestedAccelerator::Kvm,
        vm_cell_manager::cli::CliAccelerator::Hvf => RequestedAccelerator::Hvf,
        vm_cell_manager::cli::CliAccelerator::Tcg => RequestedAccelerator::Tcg,
    }
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

fn require_human_shell(json: bool) -> Result<(), CliInputError> {
    if json {
        Err(CliInputError(
            "vmcell shell is an interactive human surface and does not support --json".to_owned(),
        ))
    } else {
        Ok(())
    }
}

const MAX_SHELL_LINE_BYTES: u64 = 8 * 1024;

trait ShellCommandExecutor {
    fn execute(&mut self, line: &str) -> Result<GuestExecReport, ShellCommandFailure>;
}

struct ShellCommandFailure {
    operation_id: Option<vm_cell_manager::core::guest::GuestOperationId>,
    source: Box<dyn Error>,
}

struct EngineShellCommandExecutor<'a, P: LocalVmProvider> {
    engine: &'a CellEngine<P>,
    credentials: &'a GuestCredentials,
    cell_id: vm_cell_manager::core::cell::CellId,
    readiness: ReadinessPolicy,
    action_timeout: Duration,
    max_output_bytes: u64,
}

impl<P: LocalVmProvider> ShellCommandExecutor for EngineShellCommandExecutor<'_, P> {
    fn execute(&mut self, line: &str) -> Result<GuestExecReport, ShellCommandFailure> {
        let operation_id = std::cell::Cell::new(None);
        let result = self.engine.exec_guest_observed(
            &PowerShellDirectTransport::system(),
            self.credentials,
            GuestExecRequest {
                cell_id: self.cell_id,
                command: shell_guest_command(line, self.action_timeout, self.max_output_bytes),
                readiness: self.readiness,
            },
            |id| operation_id.set(Some(id)),
        );
        result.map_err(|source| ShellCommandFailure {
            operation_id: operation_id.get(),
            source: Box::new(source),
        })
    }
}

fn shell_guest_command(line: &str, timeout: Duration, max_output_bytes: u64) -> GuestCommand {
    GuestCommand {
        program: "powershell.exe".to_owned(),
        args: vec![
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            line.to_owned(),
        ],
        timeout,
        max_output_bytes,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellSessionEnd {
    Eof,
    ExitRequested,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShellSessionReport {
    commands_executed: u64,
    last_exit_code: i32,
    end: ShellSessionEnd,
}

fn run_shell_session<E: ShellCommandExecutor>(
    executor: &mut E,
    input: &mut impl BufRead,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    interrupted: impl Fn() -> bool,
) -> Result<ShellSessionReport, Box<dyn Error>> {
    writeln!(
        stderr,
        "vmcell shell: line-oriented PowerShell Direct; each line is an independent bounded operation"
    )?;
    writeln!(
        stderr,
        "vmcell shell: no PTY, guest stdin, Read-Host, full-screen controls, or persistent cwd/env/process state"
    )?;
    writeln!(
        stderr,
        "vmcell shell: use .exit or EOF to leave; the cell is retained"
    )?;
    let mut commands_executed = 0_u64;
    let mut last_exit_code = 0_i32;
    loop {
        if interrupted() {
            writeln!(
                stderr,
                "vmcell shell: interruption requested; cell retained"
            )?;
            return Ok(ShellSessionReport {
                commands_executed,
                last_exit_code,
                end: ShellSessionEnd::Interrupted,
            });
        }
        write!(stderr, "vmcell> ")?;
        stderr.flush()?;
        let line = match read_shell_line(input) {
            Ok(Some(line)) => line,
            Ok(None) => {
                writeln!(stderr, "\nvmcell shell: EOF; cell retained")?;
                return Ok(ShellSessionReport {
                    commands_executed,
                    last_exit_code,
                    end: ShellSessionEnd::Eof,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                writeln!(
                    stderr,
                    "\nvmcell shell: input interrupted before dispatch; cell retained"
                )?;
                return Ok(ShellSessionReport {
                    commands_executed,
                    last_exit_code,
                    end: ShellSessionEnd::Interrupted,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                writeln!(stderr, "vmcell shell: invalid input; cell retained")?;
                return Err(CliInputError(error.to_string()).into());
            }
            Err(error) => return Err(error.into()),
        };
        let directive = line.trim();
        if directive.is_empty() {
            continue;
        }
        if directive.eq_ignore_ascii_case(".exit") {
            writeln!(stderr, "vmcell shell: exit requested; cell retained")?;
            return Ok(ShellSessionReport {
                commands_executed,
                last_exit_code,
                end: ShellSessionEnd::ExitRequested,
            });
        }
        if directive.eq_ignore_ascii_case(".help") {
            writeln!(
                stderr,
                "vmcell shell: enter one PowerShell command per line; .exit leaves the cell running"
            )?;
            continue;
        }
        if interrupted() {
            writeln!(
                stderr,
                "vmcell shell: interruption observed before dispatch; cell retained"
            )?;
            return Ok(ShellSessionReport {
                commands_executed,
                last_exit_code,
                end: ShellSessionEnd::Interrupted,
            });
        }
        let report = match executor.execute(&line) {
            Ok(report) => report,
            Err(failure) => {
                let classification = classify_cli_error(failure.source.as_ref());
                let operation = failure
                    .operation_id
                    .map_or_else(|| "none".to_owned(), |id| id.to_string());
                writeln!(
                    stderr,
                    "vmcell shell: command failed code={} operation={operation}; session stopped and cell retained for status/operation reconcile",
                    classification.code
                )?;
                return Err(failure.source);
            }
        };
        commands_executed = commands_executed.saturating_add(1);
        last_exit_code = report.result.exit_code;
        stdout.write_all(report.result.stdout.as_bytes())?;
        stdout.flush()?;
        stderr.write_all(report.result.stderr.as_bytes())?;
        if !report.result.stderr.is_empty() && !report.result.stderr.ends_with('\n') {
            writeln!(stderr)?;
        }
        writeln!(
            stderr,
            "vmcell shell: operation={} exit={}",
            report.operation_id, report.result.exit_code
        )?;
        stderr.flush()?;
        if interrupted() {
            writeln!(
                stderr,
                "vmcell shell: interruption observed after the bounded operation completed; cell retained"
            )?;
            return Ok(ShellSessionReport {
                commands_executed,
                last_exit_code,
                end: ShellSessionEnd::Interrupted,
            });
        }
    }
}

#[cfg(windows)]
static CONSOLE_INTERRUPT_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
unsafe extern "system" fn console_control_handler(control: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    if matches!(control, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        CONSOLE_INTERRUPT_REQUESTED.store(true, Ordering::SeqCst);
        1
    } else {
        0
    }
}

struct ConsoleInterruptGuard;

impl ConsoleInterruptGuard {
    #[cfg(windows)]
    fn install() -> Result<Self, Box<dyn Error>> {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        CONSOLE_INTERRUPT_REQUESTED.store(false, Ordering::SeqCst);
        if unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 1) } == 0 {
            return Err(CliInputError(
                "vmcell could not install its bounded console interruption handler".to_owned(),
            )
            .into());
        }
        Ok(Self)
    }

    #[cfg(not(windows))]
    fn install() -> Result<Self, Box<dyn Error>> {
        Err(CliInputError("vmcell shell is supported only on Windows".to_owned()).into())
    }

    fn requested(&self) -> bool {
        #[cfg(windows)]
        {
            CONSOLE_INTERRUPT_REQUESTED.load(Ordering::SeqCst)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

struct RunInterruptGuard {
    #[cfg(windows)]
    _console: ConsoleInterruptGuard,
}

impl RunInterruptGuard {
    fn install() -> Result<Self, Box<dyn Error>> {
        #[cfg(windows)]
        {
            Ok(Self {
                _console: ConsoleInterruptGuard::install()?,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    fn requested(&self) -> bool {
        #[cfg(windows)]
        {
            self._console.requested()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

fn install_run_interrupt(plan: &RunExecutionPlan) -> Result<RunInterruptGuard, RunCellError> {
    install_run_interrupt_with(plan, RunInterruptGuard::install)
}

fn install_job_run_interrupt(
    plan: &RunExecutionPlan,
    job: &vm_cell_manager::core::job::JobRunContext,
) -> Result<RunInterruptGuard, RunCellError> {
    install_run_interrupt_with_job(plan, Some(job), RunInterruptGuard::install)
}

fn install_run_interrupt_with(
    plan: &RunExecutionPlan,
    install: impl FnOnce() -> Result<RunInterruptGuard, Box<dyn Error>>,
) -> Result<RunInterruptGuard, RunCellError> {
    install_run_interrupt_with_job(plan, None, install)
}

fn install_run_interrupt_with_job(
    plan: &RunExecutionPlan,
    job: Option<&vm_cell_manager::core::job::JobRunContext>,
    install: impl FnOnce() -> Result<RunInterruptGuard, Box<dyn Error>>,
) -> Result<RunInterruptGuard, RunCellError> {
    install().map_err(|_| {
        run_request_validation_error_with_job(
            plan,
            job,
            EngineError::InvalidCellRequest(
                "the bounded run interruption handler is unavailable".to_owned(),
            ),
        )
    })
}

#[cfg(windows)]
impl Drop for ConsoleInterruptGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        let _ = unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 0) };
        CONSOLE_INTERRUPT_REQUESTED.store(false, Ordering::SeqCst);
    }
}

fn read_shell_line(input: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if bytes.len().saturating_add(consumed) as u64 > MAX_SHELL_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "shell input line exceeds the supported bound",
            ));
        }
        let complete = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        bytes.extend_from_slice(&available[..consumed]);
        input.consume(consumed);
        if complete {
            break;
        }
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "shell input must not contain NUL",
        ));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "shell input is not UTF-8")
    })
}

#[cfg(windows)]
fn open_windows_console_input() -> Result<std::fs::File, Box<dyn Error>> {
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .open("CONIN$")
        .map_err(|_| {
            CliInputError(
                "vmcell shell requires an attached Windows console for command input".to_owned(),
            )
        })?)
}

#[cfg(not(windows))]
fn open_windows_console_input() -> Result<std::fs::File, Box<dyn Error>> {
    Err(CliInputError("vmcell shell is supported only from a Windows console".to_owned()).into())
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

fn write_image_dependencies(
    report: &ImageDependencyReport,
    output: &mut impl Write,
) -> std::io::Result<()> {
    writeln!(
        output,
        "image={} can_unregister={} dependencies={}",
        report.image_id,
        report.can_unregister,
        report.dependencies.len()
    )?;
    for dependency in &report.dependencies {
        writeln!(
            output,
            "  cell={} state={:?} phase={:?} blocking={}",
            dependency.cell_id, dependency.state, dependency.phase, dependency.blocking
        )?;
    }
    Ok(())
}

fn write_image_unregister(
    report: &ImageUnregisterReport,
    output: &mut impl Write,
) -> std::io::Result<()> {
    writeln!(
        output,
        "image={} metadata_removed={} bytes_deleted={} destroyed_references={}",
        report.image_id,
        report.metadata_removed,
        report.bytes_deleted,
        report.destroyed_references.len()
    )
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

fn write_state_compatibility(
    report: &StateCompatibilityReport,
    output: &mut impl Write,
) -> std::io::Result<()> {
    let status = match report.status {
        StateCompatibilityStatus::Empty => "empty",
        StateCompatibilityStatus::Compatible => "compatible",
    };
    writeln!(
        output,
        "state format={} status={} installations={} images={} cells={} operations={} artifacts={}",
        report.durable_state_format_version,
        status,
        report.counts.installations,
        report.counts.images,
        report.counts.cells,
        report.counts.guest_operations,
        report.counts.artifacts
    )
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
    let job = cell.job.as_ref().map_or_else(
        || "none".to_owned(),
        |correlation| correlation.job_id.to_string(),
    );
    writeln!(
        output,
        "cell={} provider={} image={} state={} phase={} retention={} job={} last_error={}",
        cell.id,
        cell.provider,
        cell.image.image_id,
        cell_state_name(cell.state),
        cell_phase_name(cell.phase),
        cell_retention(cell, now),
        job,
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
    let job = operation
        .job_id
        .map_or_else(|| "none".to_owned(), |job_id| job_id.to_string());
    writeln!(
        output,
        "operation={} cell={} job={} kind={} phase={} failure={} action={} updated_at={}",
        operation.id,
        operation.cell_id,
        job,
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
    let credentials = read_credentials(engine.provider_name(), credential)
        .map_err(|error| run_credential_error(&request.plan, None, error))?;
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

fn run_job_cell_guest<P: LocalVmProvider>(
    engine: &CellEngine<P>,
    credential: CredentialArgs,
    request: JobRunRequest,
    observer: &mut impl RunObserver,
) -> Result<RunCellReport, Box<dyn Error>> {
    let credentials = read_credentials(engine.provider_name(), credential)
        .map_err(|error| run_credential_error(request.plan(), Some(request.job()), error))?;
    Ok(match engine.provider_name() {
        "hyperv" => engine.run_job_cell_observed(
            &PowerShellDirectTransport::system(),
            &credentials,
            request,
            observer,
        )?,
        "qemu" => engine.run_job_cell_observed(
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

fn run_credential_error(
    plan: &RunExecutionPlan,
    job: Option<&vm_cell_manager::core::job::JobRunContext>,
    error: Box<dyn Error>,
) -> RunCellError {
    let source = match error.downcast::<CliInputError>() {
        Ok(_) => EngineError::InvalidCellRequest(
            "guest credentials do not satisfy the selected transport requirements".to_owned(),
        ),
        Err(error) => match error.downcast::<GuestIoError>() {
            Ok(error) => EngineError::Guest(*error),
            Err(_) => EngineError::Guest(GuestIoError::Transport),
        },
    };
    run_request_validation_error_with_job(plan, job, source)
}

fn guest_exit_status(exit_code: i32) -> ExitCode {
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(exit_code).unwrap_or(1))
    }
}

fn write_human_run_plan(plan: &RunExecutionPlan, output: &mut impl Write) -> std::io::Result<()> {
    let source = match plan.selection_source {
        RunSelectionSource::ExplicitCli => "explicit_cli",
        RunSelectionSource::ConfigPreference => "config_preference",
        RunSelectionSource::NativeDefault => "native_default",
    };
    writeln!(
        output,
        "vmcell: run plan: image={} provider={} accelerator={} transport={} guest={}/{} support={} source={} authority=none",
        plan.image,
        plan.provider.as_str(),
        plan.accelerator.as_str(),
        plan.guest_transport.as_str(),
        guest_os_name(plan.guest_os),
        architecture_name(plan.guest_architecture),
        plan.support_status.as_str(),
        source,
    )
}

fn write_human_job_plan(plan: &ResolvedJobPlan, output: &mut impl Write) -> std::io::Result<()> {
    let source = match plan.execution.selection_source {
        RunSelectionSource::ExplicitCli => "explicit_job_spec",
        RunSelectionSource::ConfigPreference => "config_preference",
        RunSelectionSource::NativeDefault => "native_default",
    };
    writeln!(
        output,
        "vmcell: job plan: image={} provider={} accelerator={} transport={} guest={}/{} support={} source={} cpu={} memory_mib={} ttl_seconds={} readiness_timeout_seconds={} action_timeout_seconds={} max_output_bytes={} keep={} keep_on_failure={} copy_in={} artifacts={} job_spec_sha256={} authority=none",
        plan.execution.image,
        plan.execution.provider.as_str(),
        plan.execution.accelerator.as_str(),
        plan.execution.guest_transport.as_str(),
        guest_os_name(plan.execution.guest_os),
        architecture_name(plan.execution.guest_architecture),
        plan.execution.support_status.as_str(),
        source,
        plan.resources.cpu_count,
        plan.resources.memory_mib,
        plan.resources
            .ttl_seconds
            .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        plan.timeouts.readiness_timeout_seconds,
        plan.timeouts.action_timeout_seconds,
        plan.timeouts.max_output_bytes,
        plan.cleanup.keep,
        plan.cleanup.keep_on_failure,
        plan.declared_copy_in_count,
        plan.declared_artifact_count,
        plan.job_spec_sha256,
    )
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
    if let Some(job) = &report.job {
        writeln!(
            stderr,
            "vmcell: job {}: cell {} exit={} cleanup={}",
            job.job_id,
            report.cell_id,
            report.result.exit_code,
            report.cleanup.as_str()
        )?;
        if let Some(operations) = &report.job_operations {
            writeln!(
                stderr,
                "vmcell: job operations: copy_in={} command_operation={} artifacts={}",
                operations.copy_in.len(),
                operations
                    .command_operation_id
                    .map_or_else(|| "none".to_owned(), |id| id.to_string()),
                operations.artifacts.len(),
            )
        } else {
            Ok(())
        }
    } else {
        writeln!(
            stderr,
            "vmcell: run cell {}: exit={} cleanup={}",
            report.cell_id,
            report.result.exit_code,
            report.cleanup.as_str()
        )
    }
}

struct HumanRunObserver<W: Write, F: Fn() -> bool = fn() -> bool> {
    writer: W,
    error: Option<std::io::Error>,
    interrupted: F,
}

#[cfg(test)]
fn never_interrupted() -> bool {
    false
}

#[cfg(test)]
impl<W: Write> HumanRunObserver<W, fn() -> bool> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            error: None,
            interrupted: never_interrupted,
        }
    }
}

impl<W: Write, F: Fn() -> bool> HumanRunObserver<W, F> {
    fn with_interrupt(writer: W, interrupted: F) -> Self {
        Self {
            writer,
            error: None,
            interrupted,
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

impl<W: Write, F: Fn() -> bool> RunObserver for HumanRunObserver<W, F> {
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
            RunProgressEvent::CommandCompleted {
                cell_id,
                operation_id,
                exit_code,
            } => {
                self.line(format_args!(
                    "command completed: cell={cell_id} operation={operation_id} exit={exit_code}"
                ));
            }
            RunProgressEvent::CopyInCompleted {
                cell_id,
                operation_id,
                size,
            } => {
                self.line(format_args!(
                    "copy-in completed: cell={cell_id} operation={operation_id} bytes={size}"
                ));
            }
            RunProgressEvent::ArtifactCollected {
                cell_id,
                operation_id,
                file_count,
            } => {
                self.line(format_args!(
                    "artifacts collected: cell={cell_id} operation={operation_id} files={file_count}"
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
        if (self.interrupted)() {
            RunControl::Cancel
        } else {
            RunControl::Continue
        }
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
    let timeout = Duration::from_secs(timeout_seconds);
    ReadinessPolicy {
        timeout,
        poll_interval: Duration::from_secs(2).min(timeout),
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
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let mut password = read_password_line(&mut stdin)?;
    Ok(GuestCredentials::new(
        username,
        std::mem::take(&mut *password),
    )?)
}

fn read_password_line(input: &mut impl BufRead) -> Result<Zeroizing<String>, Box<dyn Error>> {
    const MAX_PASSWORD_BYTES: usize = 4096;

    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            break;
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            bytes.extend_from_slice(&available[..=newline]);
            input.consume(newline + 1);
            break;
        }
        if bytes.len().saturating_add(available.len()) > MAX_PASSWORD_BYTES {
            return Err(CliInputError(
                "guest password stdin must contain one bounded line".to_owned(),
            )
            .into());
        }
        let consumed = available.len();
        bytes.extend_from_slice(available);
        input.consume(consumed);
    }

    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes.len() > MAX_PASSWORD_BYTES || bytes.contains(&b'\r') || bytes.contains(&b'\n') {
        return Err(
            CliInputError("guest password stdin must contain one bounded line".to_owned()).into(),
        );
    }
    let password = std::str::from_utf8(&bytes)
        .map_err(|_| CliInputError("guest password stdin must be valid UTF-8".to_owned()))?;
    Ok(Zeroizing::new(password.to_owned()))
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

    fn test_run_plan_for(
        image: &str,
        host_os: HostOs,
        guest_os: GuestOs,
        provider: ProviderId,
        accelerator: Accelerator,
        guest_transport: GuestTransportId,
    ) -> RunExecutionPlan {
        RunExecutionPlan {
            schema_version: vm_cell_manager::core::run_selection::RUN_PLAN_SCHEMA_VERSION,
            contract: vm_cell_manager::core::run_selection::RUN_PLAN_CONTRACT.to_owned(),
            image: image.parse().unwrap(),
            host_os,
            host_architecture: Architecture::X86_64,
            guest_os,
            guest_architecture: Architecture::X86_64,
            provider,
            accelerator,
            guest_transport,
            support_status: SupportStatus::Untested,
            selection_source: RunSelectionSource::NativeDefault,
            authorizing: false,
        }
    }

    fn test_run_plan() -> RunExecutionPlan {
        test_run_plan_for(
            "windows-dev",
            HostOs::Windows,
            GuestOs::Windows,
            ProviderId::Hyperv,
            Accelerator::HyperV,
            GuestTransportId::PowerShellDirect,
        )
    }

    #[test]
    fn readiness_caps_poll_interval_to_short_timeout() {
        let policy = readiness(1);

        assert_eq!(policy.timeout, Duration::from_secs(1));
        assert_eq!(policy.poll_interval, Duration::from_secs(1));
    }

    #[test]
    fn explicit_provider_overrides_configured_provider() {
        let directory = tempfile::tempdir().unwrap();
        let state_root = directory.path().join("state");
        let state = StateStore::new(state_root.clone());
        let configured = Cli::try_parse_from(["vmcell", "create", "--image", "daily"])
            .unwrap()
            .command;
        assert_eq!(
            provider_for_command(&configured, &state, ConfigProvider::Qemu).unwrap(),
            "qemu"
        );

        let explicit = Cli::try_parse_from([
            "vmcell",
            "create",
            "--image",
            "daily",
            "--provider",
            "hyperv",
        ])
        .unwrap()
        .command;
        assert_eq!(
            provider_for_command(&explicit, &state, ConfigProvider::Qemu).unwrap(),
            "hyperv"
        );

        let images = state_root.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let manifest = images.join("daily.json");
        std::fs::write(
            &manifest,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "id": "daily",
                "guest_os": "windows",
                "guest_arch": "x86_64",
                "variants": [{
                    "provider": "hyperv",
                    "disk_format": "vhdx",
                    "path": directory.path().join("base.vhdx"),
                    "sha256": "a".repeat(64),
                    "file_size": 1024
                }],
                "registered_at": "2026-08-10T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(&images, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let registered = Cli::try_parse_from(["vmcell", "image", "validate", "--id", "daily"])
            .unwrap()
            .command;
        assert_eq!(
            provider_for_command(&registered, &state, ConfigProvider::Qemu).unwrap(),
            "hyperv"
        );
    }

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
            job: None,
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
            plan: Some(test_run_plan()),
            job: None,
            job_operations: None,
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
    fn human_job_run_result_reports_safe_identity_and_counts_only() {
        let job =
            vm_cell_manager::core::job::JobRunContext::new("a".repeat(64), chrono::Utc::now())
                .unwrap()
                .result_metadata(chrono::Utc::now());
        let copy_operation_id = vm_cell_manager::core::guest::GuestOperationId::new();
        let command_operation_id = vm_cell_manager::core::guest::GuestOperationId::new();
        let artifact_operation_id = vm_cell_manager::core::guest::GuestOperationId::new();
        let report = RunCellReport {
            schema_version: 1,
            plan: Some(test_run_plan()),
            job: Some(job.clone()),
            job_operations: Some(vm_cell_manager::engine::JobOperationManifest {
                schema_version: 1,
                contract: "vmcell.job-operations.v1".to_owned(),
                copy_in: vec![vm_cell_manager::engine::JobCopyInSummary {
                    operation_id: copy_operation_id,
                    size: 17,
                }],
                command_operation_id: Some(command_operation_id),
                artifacts: vec![vm_cell_manager::engine::JobArtifactSummary {
                    operation_id: artifact_operation_id,
                    file_count: 2,
                    total_bytes: 31,
                }],
            }),
            cell_id: vm_cell_manager::core::cell::CellId::new(),
            operation_id: command_operation_id,
            outcome: vm_cell_manager::engine::RunOutcome::Success,
            result: vm_cell_manager::guest::GuestCommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                encoding: "utf-8".to_owned(),
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            },
            cleanup: vm_cell_manager::engine::RunCleanupDisposition::Destroyed,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        write_human_run_result(&report, &mut stdout, &mut stderr).unwrap();

        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains(&format!("job {}", job.job_id)));
        assert!(stderr.contains(&format!("cell {}", report.cell_id)));
        assert!(stderr.contains("copy_in=1"));
        assert!(stderr.contains(&format!("command_operation={command_operation_id}")));
        assert!(stderr.contains("artifacts=1"));
        for forbidden in [
            "job-command-secret",
            "credential-sentinel",
            "host-path-secret",
            "guest-path-secret",
        ] {
            assert!(!stderr.contains(forbidden), "leaked {forbidden}");
        }
    }

    #[test]
    fn cross_provider_run_plan_human_and_json_contracts_are_safe_and_versioned() {
        let cases = [
            (
                test_run_plan(),
                "vmcell: run plan: image=windows-dev provider=hyperv accelerator=hyper-v transport=powershell-direct guest=windows/x86_64 support=untested source=native_default authority=none\n",
                "hyperv",
                "hyper-v",
                "powershell-direct",
            ),
            (
                test_run_plan_for(
                    "linux-whpx-dev",
                    HostOs::Windows,
                    GuestOs::Linux,
                    ProviderId::Qemu,
                    Accelerator::Whpx,
                    GuestTransportId::Qga,
                ),
                "vmcell: run plan: image=linux-whpx-dev provider=qemu accelerator=whpx transport=qga guest=linux/x86_64 support=untested source=native_default authority=none\n",
                "qemu",
                "whpx",
                "qga",
            ),
            (
                test_run_plan_for(
                    "linux-kvm-dev",
                    HostOs::Linux,
                    GuestOs::Linux,
                    ProviderId::Qemu,
                    Accelerator::Kvm,
                    GuestTransportId::Qga,
                ),
                "vmcell: run plan: image=linux-kvm-dev provider=qemu accelerator=kvm transport=qga guest=linux/x86_64 support=untested source=native_default authority=none\n",
                "qemu",
                "kvm",
                "qga",
            ),
        ];

        for (plan, expected_human, provider, accelerator, transport) in cases {
            let mut human = Vec::new();
            write_human_run_plan(&plan, &mut human).unwrap();
            let human = String::from_utf8(human).unwrap();
            assert_eq!(human, expected_human);
            assert!(!human.contains("credential"));
            assert!(!human.contains("cmd.exe"));
            assert!(!human.contains("C:\\"));

            let json = serde_json::to_value(&plan).unwrap();
            assert_eq!(json["schema_version"], 1);
            assert_eq!(json["contract"], "vmcell.run-plan.v1");
            assert_eq!(json["provider"], provider);
            assert_eq!(json["accelerator"], accelerator);
            assert_eq!(json["guest_transport"], transport);
            assert_eq!(json["support_status"], "untested");
            assert_eq!(json["selection_source"], "native_default");
            assert_eq!(json["authorizing"], false);
            assert!(json.get("path").is_none());
            assert!(json.get("command").is_none());
            assert!(json.get("credential").is_none());
        }
    }

    #[test]
    fn run_credential_failure_json_retains_the_resolved_plan() {
        let plan = test_run_plan();
        let error = run_credential_error(
            &plan,
            None,
            Box::new(CliInputError("credential-secret-sentinel".to_owned())),
        );
        let classification = classify_cli_error(&error);
        let envelope = RunErrorEnvelope::new(
            classification,
            public_error_message(classification),
            error.report(),
        );
        let json = serde_json::to_value(envelope).unwrap();

        assert_eq!(json["error"]["code"], "vmcell.invalid_input");
        assert_eq!(json["run"]["stage"], "request_validation");
        assert_eq!(json["run"]["cleanup"], "nothing_created");
        assert_eq!(json["run"]["plan"]["contract"], "vmcell.run-plan.v1");
        assert_eq!(json["run"]["plan"]["image"], "windows-dev");
        assert_eq!(json["run"]["plan"]["authorizing"], false);
        assert!(
            !serde_json::to_string(&json)
                .unwrap()
                .contains("credential-secret-sentinel")
        );
    }

    #[test]
    fn interrupt_install_failure_json_retains_the_safe_resolved_plan() {
        let plan = test_run_plan();
        let error = match install_run_interrupt_with(&plan, || {
            Err(CliInputError("interrupt-secret-sentinel".to_owned()).into())
        }) {
            Ok(_) => panic!("injected interrupt installation failure should be reported"),
            Err(error) => error,
        };
        let classification = classify_cli_error(&error);
        let envelope = RunErrorEnvelope::new(
            classification,
            public_error_message(classification),
            error.report(),
        );
        let json = serde_json::to_value(envelope).unwrap();
        let serialized = serde_json::to_string(&json).unwrap();

        assert_eq!(json["error"]["code"], "vmcell.invalid_input");
        assert_eq!(json["run"]["stage"], "request_validation");
        assert_eq!(json["run"]["cleanup"], "nothing_created");
        assert_eq!(json["run"]["plan"]["contract"], "vmcell.run-plan.v1");
        assert_eq!(json["run"]["plan"]["authorizing"], false);
        assert!(!serialized.contains("interrupt-secret-sentinel"));
        assert!(!serialized.contains("credential"));
        assert!(!serialized.contains("cmd.exe"));
        assert!(!serialized.contains("C:\\"));
    }

    #[test]
    fn job_interrupt_install_failure_retains_safe_job_identity() {
        let plan = test_run_plan();
        let job =
            vm_cell_manager::core::job::JobRunContext::new("a".repeat(64), chrono::Utc::now())
                .unwrap();
        let error = match install_run_interrupt_with_job(&plan, Some(&job), || {
            Err(CliInputError("job-interrupt-secret-sentinel".to_owned()).into())
        }) {
            Ok(_) => panic!("injected interrupt installation failure should be reported"),
            Err(error) => error,
        };
        let classification = classify_cli_error(&error);
        let envelope = RunErrorEnvelope::new(
            classification,
            public_error_message(classification),
            error.report(),
        );
        let json = serde_json::to_value(envelope).unwrap();
        let serialized = serde_json::to_string(&json).unwrap();

        assert_eq!(json["run"]["stage"], "request_validation");
        assert_eq!(json["run"]["job"]["contract"], "vmcell.job-result.v1");
        assert_eq!(json["run"]["job"]["job_id"], job.job_id().to_string());
        assert_eq!(json["run"]["job"]["job_spec_sha256"], "a".repeat(64));
        assert!(json["run"].get("job_operations").is_none());
        assert!(!serialized.contains("job-interrupt-secret-sentinel"));
    }

    #[test]
    fn pre_plan_input_failure_stays_generic_while_planned_failures_are_redacted() {
        let pre_plan = CliInputError("pre-plan-secret-sentinel".to_owned());
        let classification = classify_cli_error(&pre_plan);
        let generic = serde_json::to_value(ErrorEnvelope::new(
            classification,
            public_error_message(classification),
        ))
        .unwrap();
        let generic_serialized = serde_json::to_string(&generic).unwrap();

        assert!(generic.get("run").is_none());
        assert!(generic.get("plan").is_none());
        assert!(!generic_serialized.contains("pre-plan-secret-sentinel"));

        let plan = test_run_plan();
        let planned = run_request_validation_error(
            &plan,
            EngineError::ProviderUnavailable("raw-provider-diagnostic-sentinel".to_owned()),
        );
        let classification = classify_cli_error(&planned);
        let planned_json = serde_json::to_value(RunErrorEnvelope::new(
            classification,
            public_error_message(classification),
            planned.report(),
        ))
        .unwrap();
        let planned_serialized = serde_json::to_string(&planned_json).unwrap();

        assert_eq!(
            planned_json["run"]["plan"],
            serde_json::to_value(&plan).unwrap()
        );
        assert!(!planned_serialized.contains("raw-provider-diagnostic-sentinel"));
    }

    #[test]
    fn human_run_observer_reports_safe_lifecycle_progress() {
        let image = "windows-dev".parse().unwrap();
        let cell_id = vm_cell_manager::core::cell::CellId::new();
        let operation_id = vm_cell_manager::core::guest::GuestOperationId::new();
        let mut observer = HumanRunObserver::new(Vec::new());
        for event in [
            RunProgressEvent::ImageVerified { image },
            RunProgressEvent::CellCreated { cell_id },
            RunProgressEvent::ProviderStarted { cell_id },
            RunProgressEvent::GuestReady { cell_id },
            RunProgressEvent::CommandCompleted {
                cell_id,
                operation_id,
                exit_code: 0,
            },
            RunProgressEvent::CopyInCompleted {
                cell_id,
                operation_id,
                size: 17,
            },
            RunProgressEvent::ArtifactCollected {
                cell_id,
                operation_id,
                file_count: 1,
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
        assert!(output.contains(&format!(
            "vmcell: command completed: cell={cell_id} operation={operation_id} exit=0"
        )));
        assert!(output.contains(&format!(
            "vmcell: copy-in completed: cell={cell_id} operation={operation_id} bytes=17"
        )));
        assert!(output.contains(&format!(
            "vmcell: artifacts collected: cell={cell_id} operation={operation_id} files=1"
        )));
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
    fn run_observer_samples_interruption_at_the_next_durable_stage_boundary() {
        let cell_id = vm_cell_manager::core::cell::CellId::new();
        let mut observer = HumanRunObserver::with_interrupt(Vec::new(), || true);
        assert_eq!(
            observer.observe(&RunProgressEvent::ProviderStarted { cell_id }),
            RunControl::Cancel
        );
        let output = String::from_utf8(observer.finish().unwrap()).unwrap();
        assert!(output.contains(&format!("provider started: {cell_id}")));
    }

    #[test]
    fn password_pipe_reads_one_bounded_line_without_waiting_for_eof() {
        let mut input = std::io::BufReader::new(std::io::Cursor::new(
            b"credential-sentinel\r\nignored-after-first-line".to_vec(),
        ));
        let password = read_password_line(&mut input).unwrap();
        assert_eq!(&**password, "credential-sentinel");
        let mut remainder = String::new();
        input.read_to_string(&mut remainder).unwrap();
        assert_eq!(remainder, "ignored-after-first-line");

        let mut oversized = std::io::BufReader::new(std::io::Cursor::new(vec![b'x'; 4097]));
        let error = read_password_line(&mut oversized).unwrap_err();
        assert!(error.downcast_ref::<CliInputError>().is_some());

        let mut invalid = std::io::BufReader::new(std::io::Cursor::new(vec![0xff, b'\n']));
        let error = read_password_line(&mut invalid).unwrap_err();
        assert!(error.downcast_ref::<CliInputError>().is_some());
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
        let job_id = vm_cell_manager::core::job::JobId::new();
        operation.job_id = Some(job_id);
        let mut output = Vec::new();

        write_guest_operation(&operation, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("phase=transport_active"));
        assert!(output.contains("action=manual_review"));
        assert!(output.contains(&format!("job={job_id}")));
        assert_eq!(
            guest_operation_required_action(operation.phase),
            RequiredAction::ManualReview
        );

        let mut correlated_cell = test_cell(CellState::Running, None);
        correlated_cell.job = Some(
            vm_cell_manager::core::job::JobCorrelation::new(
                job_id,
                "a".repeat(64),
                chrono::Utc::now(),
            )
            .unwrap(),
        );
        let mut cell_output = Vec::new();
        write_cell_summary(&correlated_cell, chrono::Utc::now(), &mut cell_output).unwrap();
        assert!(
            String::from_utf8(cell_output)
                .unwrap()
                .contains(&format!("job={job_id}"))
        );

        let inspection = CellInspection {
            schema_version: 1,
            cell: correlated_cell,
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

    struct MockShellExecutor {
        lines: Vec<String>,
        results: std::collections::VecDeque<
            Result<GuestExecReport, vm_cell_manager::guest::GuestIoError>,
        >,
    }

    impl ShellCommandExecutor for MockShellExecutor {
        fn execute(&mut self, line: &str) -> Result<GuestExecReport, ShellCommandFailure> {
            self.lines.push(line.to_owned());
            self.results
                .pop_front()
                .expect("mock shell result should exist")
                .map_err(|source| ShellCommandFailure {
                    operation_id: Some(vm_cell_manager::core::guest::GuestOperationId::new()),
                    source: Box::new(source),
                })
        }
    }

    fn shell_result(exit_code: i32, stdout: &str, stderr: &str) -> GuestExecReport {
        GuestExecReport {
            schema_version: 1,
            operation_id: vm_cell_manager::core::guest::GuestOperationId::new(),
            cell_id: vm_cell_manager::core::cell::CellId::new(),
            result: vm_cell_manager::guest::GuestCommandResult {
                exit_code,
                stdout: stdout.to_owned(),
                stderr: stderr.to_owned(),
                encoding: "utf-8".to_owned(),
                stdout_bytes: stdout.len() as u64,
                stderr_bytes: stderr.len() as u64,
                truncated: false,
            },
        }
    }

    #[test]
    fn shell_is_line_oriented_forwards_streams_and_retains_nonzero_status() {
        let mut executor = MockShellExecutor {
            lines: Vec::new(),
            results: std::collections::VecDeque::from([
                Ok(shell_result(0, "first\n", "")),
                Ok(shell_result(7, "", "second-error\n")),
            ]),
        };
        let mut input =
            std::io::Cursor::new(b"  Write-Output first  \nWrite-Error second\n.exit\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let report = run_shell_session(&mut executor, &mut input, &mut stdout, &mut stderr, || {
            false
        })
        .expect("mock shell should finish");

        assert_eq!(
            executor.lines,
            ["  Write-Output first  ", "Write-Error second"]
        );
        assert_eq!(stdout, b"first\n");
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("line-oriented PowerShell Direct"));
        assert!(stderr.contains("no PTY, guest stdin, Read-Host, full-screen controls"));
        assert!(stderr.contains("second-error"));
        assert!(stderr.contains("exit=7"));
        assert!(stderr.contains("cell retained"));
        assert_eq!(report.commands_executed, 2);
        assert_eq!(report.last_exit_code, 7);
        assert_eq!(report.end, ShellSessionEnd::ExitRequested);
    }

    #[test]
    fn shell_adapter_uses_fixed_powershell_program_and_typed_arguments() {
        let command = shell_guest_command("  Write-Output 'a b'  ", Duration::from_secs(17), 4096);
        assert_eq!(command.program, "powershell.exe");
        assert_eq!(
            command.args,
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "  Write-Output 'a b'  ",
            ]
        );
        assert_eq!(command.timeout, Duration::from_secs(17));
        assert_eq!(command.max_output_bytes, 4096);
        command.validate().unwrap();
    }

    #[test]
    fn shell_transport_failures_stop_without_cleanup_or_replay() {
        for failure in [
            vm_cell_manager::guest::GuestIoError::Timeout,
            vm_cell_manager::guest::GuestIoError::AuthenticationFailed,
            vm_cell_manager::guest::GuestIoError::OwnershipChanged,
            vm_cell_manager::guest::GuestIoError::SessionFailed,
            vm_cell_manager::guest::GuestIoError::Transport,
        ] {
            let mut executor = MockShellExecutor {
                lines: Vec::new(),
                results: std::collections::VecDeque::from([Err(failure)]),
            };
            let mut input = std::io::Cursor::new(b"Get-Date\nWrite-Output never\n");
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            assert!(
                run_shell_session(&mut executor, &mut input, &mut stdout, &mut stderr, || {
                    false
                },)
                .is_err()
            );
            assert_eq!(executor.lines, ["Get-Date"]);
            assert!(stdout.is_empty());
            let stderr = String::from_utf8(stderr).unwrap();
            assert!(stderr.contains("cell retained for status/operation reconcile"));
            assert!(stderr.contains("operation="));
            assert!(!stderr.contains("credential-sentinel"));
        }
    }

    struct InterruptedShellInput;

    impl Read for InterruptedShellInput {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        }
    }

    impl BufRead for InterruptedShellInput {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        }

        fn consume(&mut self, _amount: usize) {}
    }

    #[test]
    fn shell_input_interruption_is_safe_before_dispatch_and_retains_cell() {
        let mut executor = MockShellExecutor {
            lines: Vec::new(),
            results: std::collections::VecDeque::new(),
        };
        let mut input = InterruptedShellInput;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let report = run_shell_session(&mut executor, &mut input, &mut stdout, &mut stderr, || {
            false
        })
        .expect("pre-dispatch interruption should be classified");

        assert_eq!(report.end, ShellSessionEnd::Interrupted);
        assert_eq!(report.commands_executed, 0);
        assert!(executor.lines.is_empty());
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("input interrupted before dispatch; cell retained")
        );
    }

    struct InterruptAfterActionShell {
        requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl ShellCommandExecutor for InterruptAfterActionShell {
        fn execute(&mut self, _line: &str) -> Result<GuestExecReport, ShellCommandFailure> {
            self.requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(shell_result(0, "completed\n", ""))
        }
    }

    #[test]
    fn shell_console_interrupt_is_observed_after_bounded_action_without_replay() {
        let requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut executor = InterruptAfterActionShell {
            requested: requested.clone(),
        };
        let mut input = std::io::Cursor::new(b"Get-Date\nWrite-Output never\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let report = run_shell_session(&mut executor, &mut input, &mut stdout, &mut stderr, || {
            requested.load(std::sync::atomic::Ordering::SeqCst)
        })
        .expect("completed action should be reported before interruption exit");

        assert_eq!(report.commands_executed, 1);
        assert_eq!(report.end, ShellSessionEnd::Interrupted);
        assert_eq!(stdout, b"completed\n");
        assert!(String::from_utf8(stderr).unwrap().contains(
            "interruption observed after the bounded operation completed; cell retained"
        ));
    }

    #[test]
    fn shell_input_is_strict_utf8_and_bounded() {
        assert!(matches!(
            read_shell_line(&mut std::io::Cursor::new([0xff, b'\n'])),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData
        ));
        let oversized = vec![b'a'; MAX_SHELL_LINE_BYTES as usize + 1];
        assert!(matches!(
            read_shell_line(&mut std::io::Cursor::new(oversized)),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData
        ));
        assert!(matches!(
            read_shell_line(&mut std::io::Cursor::new(b"Get-Date\0\n")),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData
        ));

        let mut executor = MockShellExecutor {
            lines: Vec::new(),
            results: std::collections::VecDeque::new(),
        };
        let mut input = std::io::Cursor::new([0xff, b'\n']);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = run_shell_session(&mut executor, &mut input, &mut stdout, &mut stderr, || {
            false
        })
        .unwrap_err();
        let classification = classify_cli_error(error.as_ref());
        assert_eq!(classification.code, "vmcell.invalid_input");
        assert_eq!(classification.exit_code, CliExitCode::InvalidInput);
        assert!(executor.lines.is_empty());
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("invalid input; cell retained")
        );
    }

    #[test]
    fn shell_rejects_json_before_any_interactive_input() {
        assert!(require_human_shell(false).is_ok());
        assert_eq!(
            require_human_shell(true).unwrap_err().to_string(),
            "invalid CLI input: vmcell shell is an interactive human surface and does not support --json"
        );
    }
}
