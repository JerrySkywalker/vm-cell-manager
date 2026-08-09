use std::error::Error;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, error::ErrorKind};
use serde::Serialize;
use vm_cell_manager::cli::{
    ArtifactCommand, Cli, CliInputError, CliProvider, Command, CredentialArgs, DoctorReport,
    ErrorEnvelope, GuestOperationCommand, ImageCommand, ListEnvelope, ProviderCommand,
    RunErrorEnvelope, classify_cli_error, public_error_message,
};
use vm_cell_manager::core::cell::CellSpec;
use vm_cell_manager::engine::{
    ArtifactCollectRequest, ArtifactPruneRequest, CellEngine, EngineError, GuestCopyInRequest,
    GuestCopyOutRequest, GuestExecRequest, RegisterImageRequest, RunCellError, RunCellReport,
    RunCellRequest, RunCleanupPolicy, RunControl, RunObserver, RunProgressEvent,
};
use vm_cell_manager::guest::powershell_direct::PowerShellDirectTransport;
use vm_cell_manager::guest::qga::QemuGuestAgentTransport;
use vm_cell_manager::guest::{GuestCommand, GuestCredentials, ReadinessPolicy};
use vm_cell_manager::providers::hyperv::HyperVProvider;
use vm_cell_manager::providers::qemu::QemuProvider;
use vm_cell_manager::providers::{LocalVmProvider, builtin_provider_probes};
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
                println!("vmcell doctor");
                println!("host_os={}", report.host_os);
                println!("host_arch={}", report.host_arch);
                println!("state_root={}", report.state_root.display());
                for provider in &report.providers {
                    println!(
                        "provider={} available={} detail={}",
                        provider.name, provider.available, provider.detail
                    );
                }
            })?;
        }
        Command::Provider {
            command: ProviderCommand::List,
        } => {
            let response = ListEnvelope::new(builtin_provider_probes());
            emit(&response, cli.json, || {
                for probe in &response.items {
                    println!(
                        "{}\t{}\t{}",
                        probe.name,
                        if probe.available {
                            "available"
                        } else {
                            "unavailable"
                        },
                        probe.detail
                    );
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
                    println!("{}\t{:?}", inspection.cell.id, inspection.reconciliation);
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
                emit(&image, json, || println!("registered image {}", image.id))?;
            }
            ImageCommand::List => {
                let response = ListEnvelope::new(engine.list_images()?);
                emit(&response, json, || {
                    for image in &response.items {
                        println!("{}", image.id);
                    }
                })?;
            }
            ImageCommand::Inspect { id } => {
                let image = engine.inspect_image(&id)?;
                emit(&image, json, || println!("{image:#?}"))?;
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
                    println!("{}\t{:?}", cell.id, cell.state);
                }
            })?;
        }
        Command::Inspect { cell_id } => {
            let inspection = engine.inspect_cell(cell_id)?;
            emit(&inspection, json, || println!("{inspection:#?}"))?;
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
                emit(&inspection, json, || println!("{inspection:#?}"))?;
            } else {
                let response = ListEnvelope::new(engine.reconcile_all()?);
                emit(&response, json, || {
                    for inspection in &response.items {
                        println!("{}\t{:?}", inspection.cell.id, inspection.reconciliation);
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
                        println!("{}\t{:?}", operation.id, operation.phase);
                    }
                })?;
            }
            GuestOperationCommand::Inspect { operation_id } => {
                let operation = engine.inspect_guest_operation(operation_id)?;
                emit(&operation, json, || println!("{operation:#?}"))?;
            }
            GuestOperationCommand::Reconcile { operation_id } => {
                let report = engine.reconcile_guest_operation(operation_id)?;
                emit(&report, json, || println!("{report:#?}"))?;
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
        Command::Doctor | Command::Provider { .. } => {
            unreachable!("handled before engine creation")
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn provider_for_command(command: &Command, state: &StateStore) -> Result<String, Box<dyn Error>> {
    let provider = match command {
        Command::Image {
            command: ImageCommand::Add { provider, .. },
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
        Command::List
        | Command::Image { .. }
        | Command::Operation { .. }
        | Command::Doctor
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
}
