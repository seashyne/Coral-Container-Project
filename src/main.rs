use nix::sched::{unshare, CloneFlags};
use nix::unistd::{gethostname, fork, ForkResult, sethostname, chdir, pivot_root};
use nix::sys::wait::waitpid;
use nix::mount::{mount, MsFlags};
use std::process;
use std::path::Path;
use std::fs::{create_dir, write};

fn setup_cgroups(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    let cgroup_name = format!("coral-{}", pid);
    let cgroup_path = Path::new("/sys/fs/cgroup").join(&cgroup_name);

    if !cgroup_path.exists() {
        create_dir(&cgroup_path)?;
    }

    // จำกัด Memory ไว้ที่ 100MB (100 * 1024 * 1024 bytes)
    write(cgroup_path.join("memory.max"), "104857600")?;

    // เพิ่ม PID ของ Child process เข้าไปใน Cgroup เพื่อเริ่มคุมทรัพยากร
    write(cgroup_path.join("cgroup.procs"), pid.to_string())?;

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rootfs = "rootfs";

    unshare(CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS | CloneFlags::CLONE_NEWNS)
        .map_err(|e| format!("Failed to unshare: {}", e))?;

    match unsafe { fork() }.map_err(|e| format!("Failed to fork: {}", e))? {
        ForkResult::Parent { child } => {
            println!("Parent: Setting up cgroups for child PID {}", child);
            setup_cgroups(child.as_raw() as u32)?;
            
            println!("Parent: Child PID is {}", child);
            waitpid(child, None).map_err(|e| format!("Failed to wait for child: {}", e))?;
        }
        ForkResult::Child => {
            sethostname("coral-container").map_err(|e| format!("Failed to set hostname: {}", e))?;

            // 1. Bind mount rootfs to itself to make it a mount point
            mount(Some(rootfs), rootfs, None::<&str>, MsFlags::MS_BIND | MsFlags::MS_REC, None::<&str>)
                .map_err(|e| format!("Failed to bind mount rootfs: {}", e))?;

            // 2. pivot_root
            pivot_root(rootfs, rootfs).map_err(|e| format!("Failed to pivot_root: {}", e))?;

            // 3. chdir to /
            chdir(Path::new("/")).map_err(|e| format!("Failed to chdir: {}", e))?;

            println!("Child: Filesystem isolated. Current root: {:?}", std::env::current_dir()?);
        }
    }
    Ok(())
}
