//! `coral network` — built-in and user-defined container network management.

use super::Ctx;
use crate::args::NetworkAction;
use anyhow::Result;
use std::process::ExitCode;

pub(crate) fn network(_ctx: &Ctx, args: crate::args::NetworkArgs) -> Result<ExitCode> {
    match args.action.unwrap_or(NetworkAction::Ls) {
        NetworkAction::Create {
            name,
            driver,
            subnet,
        } => {
            let net = coral_net::create_network(&name, Some(&driver), subnet.as_deref())?;
            println!(
                "Created network '{}' (driver={}, subnet={}, gateway={})",
                net.name, net.driver, net.subnet, net.gateway
            );
        }
        NetworkAction::Ls => {
            let nets = coral_net::list_networks();
            let egress = coral_net::detect_egress_backend()
                .map(|(b, _)| b.binary_name())
                .unwrap_or("none (install pasta or slirp4netns for rootless egress)");
            println!(
                "{:<20} {:<10} {:<18} {:<16} SCOPE",
                "NAME", "DRIVER", "SUBNET", "GATEWAY"
            );
            for n in nets {
                println!(
                    "{:<20} {:<10} {:<18} {:<16} {}",
                    n.name,
                    n.driver,
                    if n.subnet.is_empty() { "-" } else { &n.subnet },
                    if n.gateway.is_empty() {
                        "-"
                    } else {
                        &n.gateway
                    },
                    if n.builtin { "built-in" } else { "local" }
                );
            }
            println!("Rootless bridge egress backend: {egress}");
        }
        NetworkAction::Inspect { name } => {
            let net = coral_net::find_network(&name)
                .ok_or_else(|| anyhow::anyhow!("network '{name}' not found"))?;
            println!("{}", serde_json::to_string_pretty(&net)?);
        }
        NetworkAction::Rm { name } => {
            coral_net::remove_network(&name)?;
            println!("Removed network '{name}'");
        }
    }
    Ok(ExitCode::SUCCESS)
}
