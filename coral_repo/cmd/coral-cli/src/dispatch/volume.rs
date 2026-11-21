//! `coral volume` — persistent named volume management.

use super::Ctx;
use crate::args::VolumeAction;
use crate::catalog;
use anyhow::{Context, Result};
use coral_daemon::images;
use coral_runtime::rootfs::{
    default_volumes_dir, ensure_named_volume, list_named_volumes, remove_named_volume,
};
use std::process::ExitCode;

pub(crate) fn volume(_ctx: &Ctx, args: crate::args::VolumeArgs) -> Result<ExitCode> {
    let vol_root = default_volumes_dir();
    match args.action.unwrap_or(VolumeAction::Ls) {
        VolumeAction::Create { name } => {
            let _mountpoint = ensure_named_volume(&vol_root, &name)
                .with_context(|| format!("create volume '{name}'"))?;
            println!("{name}");
        }
        VolumeAction::Ls => {
            let vols = list_named_volumes(&vol_root)
                .with_context(|| format!("list volumes in {}", vol_root.display()))?;
            if vols.is_empty() {
                println!("No named volumes in {}.", vol_root.display());
            } else {
                println!("{:<24} {:>10}  MOUNTPOINT", "VOLUME NAME", "SIZE");
                for (vname, mountpoint) in vols {
                    let size = images::dir_size(&mountpoint);
                    println!(
                        "{:<24} {:>10}  {}",
                        vname,
                        catalog::format_bytes(size),
                        mountpoint.display()
                    );
                }
            }
        }
        VolumeAction::Inspect { name } => {
            let vols = list_named_volumes(&vol_root)?;
            let (vname, mountpoint) = vols
                .into_iter()
                .find(|(vn, _)| vn == &name)
                .ok_or_else(|| anyhow::anyhow!("volume '{name}' not found"))?;
            let size = images::dir_size(&mountpoint);
            let info = serde_json::json!({
                "name": vname,
                "mountpoint": mountpoint,
                "size": size,
            });
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        VolumeAction::Rm { name } => {
            remove_named_volume(&vol_root, &name)
                .with_context(|| format!("remove volume '{name}'"))?;
            println!("Removed volume '{name}'");
        }
    }
    Ok(ExitCode::SUCCESS)
}
