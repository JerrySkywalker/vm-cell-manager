use clap::Parser;
use vm_cell_manager::cli::{Cli, Command};
use vm_cell_manager::providers::builtin_provider_probes;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Doctor => {
            let report = vm_cell_manager::cli::DoctorReport::collect();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("vmcell doctor");
                println!("host_os={}", report.host_os);
                println!("host_arch={}", report.host_arch);
                println!("state_root={}", report.state_root.display());
                for provider in report.providers {
                    println!(
                        "provider={} available={} detail={}",
                        provider.name, provider.available, provider.detail
                    );
                }
            }
        }
        Command::ProviderList => {
            let probes = builtin_provider_probes();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&probes)?);
            } else {
                for probe in probes {
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
            }
        }
    }

    Ok(())
}
