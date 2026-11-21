//! Bidirectional file/directory copy between host and container (`coral cp <SRC> <DST>`),
//! supporting both live running containers (via `setns` exec stream) and stopped containers
//! (OverlayFS CoW upperdir + base rootfs merge).

mod endpoint;
mod fs;
mod live;

pub use endpoint::{CpEndpoint, parse_cp_endpoint};

use anyhow::{Result, bail};
use std::path::Path;

pub async fn cmd_cp(addr: &str, state_root: &Path, src: &str, dst: &str) -> Result<()> {
    match (parse_cp_endpoint(src), parse_cp_endpoint(dst)) {
        (CpEndpoint::Host(host_src), CpEndpoint::Container { id, path }) => {
            if !host_src.exists() {
                bail!("host source '{}' does not exist", host_src.display());
            }
            let rel = endpoint::sanitize_container_rel_path(&path)?;
            let target = endpoint::resolve_container_target(addr, state_root, &id).await?;
            fs::copy_host_to_container(&target, &host_src, &rel)?;
            println!(
                "Copied '{}' -> '{id}:/{}'",
                host_src.display(),
                rel.display()
            );
            Ok(())
        }
        (CpEndpoint::Container { id, path }, CpEndpoint::Host(host_dst)) => {
            let rel = endpoint::sanitize_container_rel_path(&path)?;
            let target = endpoint::resolve_container_target(addr, state_root, &id).await?;
            fs::copy_container_to_host(&target, &rel, &host_dst)?;
            println!(
                "Copied '{id}:/{}' -> '{}'",
                rel.display(),
                host_dst.display()
            );
            Ok(())
        }
        (CpEndpoint::Host(_), CpEndpoint::Host(_)) => {
            bail!("one of SRC or DST must specify a container path (`<CONTAINER_ID>:<PATH>`)")
        }
        (CpEndpoint::Container { .. }, CpEndpoint::Container { .. }) => {
            bail!("copying directly between two containers is not supported; copy via host")
        }
    }
}
