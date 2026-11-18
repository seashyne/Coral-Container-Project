use nix::sched::{unshare, CloneFlags};
use nix::unistd::{gethostname, fork, ForkResult, sethostname};
use nix::sys::wait::waitpid;
use std::process;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hostname = gethostname().map_err(|e| format!("Failed to get hostname: {}", e))?;
    println!("Before unshare: PID: {}, Hostname: {:?}", process::id(), hostname);

    unshare(CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS)
        .map_err(|e| format!("Failed to unshare: {}", e))?;

    match unsafe { fork() }.map_err(|e| format!("Failed to fork: {}", e))? {
        ForkResult::Parent { child } => {
            println!("Parent: Child PID is {}", child);
            waitpid(child, None).map_err(|e| format!("Failed to wait for child: {}", e))?;
            println!("Parent: Hostname: {:?}", gethostname().map_err(|e| format!("Failed to get hostname: {}", e))?);
        }
        ForkResult::Child => {
            sethostname("coral-container").map_err(|e| format!("Failed to set hostname: {}", e))?;
            let pid = process::id();
            let child_hostname = gethostname().map_err(|e| format!("Failed to get hostname: {}", e))?;
            println!("Child: PID: {}, Hostname: {:?}", pid, child_hostname);
            if pid != 1 {
                eprintln!("Error: Child PID is not 1");
                process::exit(1);
            }
        }
    }
    Ok(())
}
