use std::error::Error;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;
use vm_cell_manager::cli::{
    Cli, Command, DoctorReport, ErrorBody, ErrorEnvelope, ImageCommand, ListEnvelope,
};
use vm_cell_manager::core::cell::CellSpec;
use vm_cell_manager::engine::{CellEngine, RegisterImageRequest};
use vm_cell_manager::providers::builtin_provider_probes;
use vm_cell_manager::providers::hyperv::HyperVProvider;
use vm_cell_manager::state::StateStore;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                let message = error.to_string();
                let envelope = ErrorEnvelope {
                    schema_version: 1,
                    error: ErrorBody {
                        category: "operation_failed",
                        message: &message,
                    },
                };
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&envelope)
                        .unwrap_or_else(|_| "{\"schema_version\":1}".to_owned())
                );
            } else {
                eprintln!("vmcell: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let state_root = cli.state_root.clone();
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
        Command::ProviderList => {
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
        command => {
            let state = StateStore::new(state_root.unwrap_or_else(StateStore::default_root));
            let engine = CellEngine::new(state, HyperVProvider::system());
            run_m1(command, cli.json, &engine)?;
        }
    }
    Ok(())
}

fn run_m1(
    command: Command,
    json: bool,
    engine: &CellEngine<HyperVProvider>,
) -> Result<(), Box<dyn Error>> {
    match command {
        Command::Image { command } => match command {
            ImageCommand::Add {
                id,
                path,
                guest_os,
                guest_arch,
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
        } => {
            let cell = engine.create_cell(CellSpec {
                image,
                provider: Some("hyperv".to_owned()),
                cpu_count,
                memory_mib,
                ttl_seconds: None,
            })?;
            emit(&cell, json, || {
                println!("created cell {} ({:?})", cell.id, cell.state)
            })?;
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
        Command::Doctor | Command::ProviderList => unreachable!("handled before engine creation"),
    }
    Ok(())
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
