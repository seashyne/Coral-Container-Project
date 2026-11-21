use crate::settings::{Mode, Settings};
use coral_api::VersionRequest;
use coral_api::runtime_service_client::RuntimeServiceClient;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

pub struct SpawnedDaemon {
    pub child: Child,
    pub mode: Mode,
}

fn endpoint_url(endpoint: &str) -> String {
    format!("http://{endpoint}")
}

pub async fn probe(endpoint: &str) -> bool {
    let Ok(mut client) = RuntimeServiceClient::connect(endpoint_url(endpoint)).await else {
        return false;
    };
    client
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .is_ok()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn resolve_binary_with(
    bin: &str,
    path_env: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
    system_fallbacks: &[&Path],
) -> String {
    let trimmed = bin.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let as_path = Path::new(trimmed);
    if trimmed.contains('/') || trimmed.contains('\\') || as_path.is_absolute() {
        return trimmed.to_string();
    }

    let local_bin = home.map(|h| h.join(".local/bin"));
    let mut custom_path_dirs = Vec::new();
    let mut system_path_dirs = Vec::new();
    if let Some(raw_path) = path_env {
        let mut seen_system = false;
        for dir in std::env::split_paths(raw_path) {
            if dir == Path::new("/usr/local/bin")
                || dir == Path::new("/usr/bin")
                || dir == Path::new("/bin")
            {
                seen_system = true;
            }
            if seen_system {
                system_path_dirs.push(dir);
            } else {
                custom_path_dirs.push(dir);
            }
        }
    }

    let mut candidates = Vec::new();
    candidates.extend(custom_path_dirs);
    if let Some(lb) = &local_bin {
        candidates.push(lb.clone());
    }
    candidates.extend(system_path_dirs);
    for dir in system_fallbacks {
        candidates.push(dir.to_path_buf());
    }

    for dir in candidates {
        let candidate = dir.join(trimmed);
        if is_executable_file(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }

    trimmed.to_string()
}

fn resolve_binary(bin: &str) -> String {
    resolve_binary_with(
        bin,
        std::env::var_os("PATH").as_deref(),
        home_dir().as_deref(),
        &[Path::new("/usr/local/bin"), Path::new("/usr/bin")],
    )
}

fn resolve_default_rootfs_in(xdg_state_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let state_base = xdg_state_home
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/state")))?;

    for tag in ["alpine-3.20", "alpine-latest"] {
        let candidate = state_base.join("coral/images").join(tag).join("rootfs");
        if candidate.is_dir() && candidate.join("bin/sh").symlink_metadata().is_ok() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_default_rootfs() -> Option<PathBuf> {
    resolve_default_rootfs_in(
        std::env::var_os("XDG_STATE_HOME")
            .as_deref()
            .map(Path::new),
        home_dir().as_deref(),
    )
}

fn merged_path_with_local_bin(
    path_env: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
) -> Option<OsString> {
    let local_bin = home.map(|h| h.join(".local/bin"))?;
    if !local_bin.is_dir() {
        return path_env.map(OsString::from);
    }
    let mut entries = vec![local_bin];
    if let Some(raw_path) = path_env {
        for dir in std::env::split_paths(raw_path) {
            if !entries.contains(&dir) {
                entries.push(dir);
            }
        }
    }
    std::env::join_paths(entries).ok()
}

fn local_command(settings: &Settings) -> Command {
    let daemon_bin = resolve_binary(&settings.daemon_bin);
    let mut cmd = Command::new(&daemon_bin);
    cmd.arg("--listen").arg(&settings.endpoint);

    if let Some(merged_path) = merged_path_with_local_bin(
        std::env::var_os("PATH").as_deref(),
        home_dir().as_deref(),
    ) {
        cmd.env("PATH", merged_path);
    }

    if !settings.coral_bin.trim().is_empty() {
        cmd.arg("--coral-bin")
            .arg(resolve_binary(&settings.coral_bin));
    }
    if !settings.state_dir.trim().is_empty() {
        cmd.arg("--state-dir").arg(&settings.state_dir);
    }
    if !settings.images_dir.trim().is_empty() {
        cmd.arg("--images-dir").arg(&settings.images_dir);
    }
    if !settings.rootfs.trim().is_empty() {
        cmd.arg("--default-rootfs").arg(&settings.rootfs);
    } else if let Some(default_rootfs) = resolve_default_rootfs() {
        cmd.arg("--default-rootfs").arg(default_rootfs);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

#[cfg(windows)]
fn wsl_command(settings: &Settings) -> Command {
    let mut cmd = Command::new("wsl.exe");
    if !settings.wsl_distro.trim().is_empty() {
        cmd.arg("-d").arg(&settings.wsl_distro);
    }
    let mut inner = format!(
        "{} --listen {} --coral-bin {}",
        shell_quote(&settings.daemon_bin),
        shell_quote(&settings.endpoint),
        shell_quote(&settings.coral_bin)
    );
    if !settings.state_dir.trim().is_empty() {
        inner.push_str(&format!(
            " --state-dir {}",
            shell_quote(&settings.state_dir)
        ));
    }
    if !settings.images_dir.trim().is_empty() {
        inner.push_str(&format!(
            " --images-dir {}",
            shell_quote(&settings.images_dir)
        ));
    }
    if !settings.rootfs.trim().is_empty() {
        inner.push_str(&format!(
            " --default-rootfs {}",
            shell_quote(&settings.rootfs)
        ));
    }
    cmd.arg("--").arg("sh").arg("-c").arg(inner);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

#[cfg(windows)]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Ensures a daemon is reachable at the configured endpoint, spawning one if needed.
/// Returns the spawned child when this call started the daemon (None when attaching
/// to an already-running daemon or when the mode cannot spawn).
pub async fn ensure(settings: &Settings) -> Result<Option<SpawnedDaemon>, String> {
    if probe(&settings.endpoint).await {
        return Ok(None);
    }

    let mut cmd = match settings.mode {
        Mode::Local => {
            if cfg!(windows) {
                return Err(
                    "Local mode needs a Linux daemon. Use WSL2 mode or a remote endpoint on Windows."
                        .into(),
                );
            }
            local_command(settings)
        }
        Mode::Wsl2 => {
            #[cfg(windows)]
            {
                wsl_command(settings)
            }
            #[cfg(not(windows))]
            {
                return Err("WSL2 mode is only available on Windows.".into());
            }
        }
        Mode::Remote => {
            return Err(format!(
                "no daemon reachable at {} — start coral-daemon on the remote host first",
                settings.endpoint
            ));
        }
    };

    let resolved_bin = if matches!(settings.mode, Mode::Local) {
        resolve_binary(&settings.daemon_bin)
    } else {
        settings.daemon_bin.clone()
    };

    let child = cmd
        .spawn()
        .map_err(|e| format!("spawn daemon ({resolved_bin}): {e}"))?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        if probe(&settings.endpoint).await {
            return Ok(Some(SpawnedDaemon {
                child,
                mode: settings.mode,
            }));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let mut child = child;
    let _ = child.kill().await;
    Err(format!(
        "daemon did not become ready at {} within 20s",
        settings.endpoint
    ))
}

#[cfg(test)]
mod tests {
    use super::{merged_path_with_local_bin, resolve_binary_with, resolve_default_rootfs_in};
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "coral-desktop-test-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn explicit_binary_path_is_preserved() {
        assert_eq!(
            resolve_binary_with(
                "/opt/custom/coral-daemon",
                Some(OsStr::new("/usr/bin")),
                None,
                &[],
            ),
            "/opt/custom/coral-daemon"
        );
    }

    #[test]
    fn prefers_home_local_bin_over_system_fallback() {
        let root = make_temp_dir("local-bin");
        let home = root.join("home");
        let local_bin = home.join(".local/bin");
        let sys_bin = root.join("sys-bin");
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&sys_bin).unwrap();
        let expected = local_bin.join("coral-daemon");
        fs::write(&expected, b"").unwrap();
        fs::write(sys_bin.join("coral-daemon"), b"").unwrap();

        let resolved = resolve_binary_with(
            "coral-daemon",
            Some(OsStr::new("/nonexistent")),
            Some(&home),
            &[sys_bin.as_path()],
        );
        assert_eq!(resolved, expected.to_string_lossy());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn custom_path_override_beats_home_local_bin() {
        let root = make_temp_dir("custom-bin");
        let home = root.join("home");
        let local_bin = home.join(".local/bin");
        let custom_bin = root.join("custom-bin");
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&custom_bin).unwrap();
        let expected = custom_bin.join("coral-daemon");
        fs::write(&expected, b"").unwrap();
        fs::write(local_bin.join("coral-daemon"), b"").unwrap();

        let resolved = resolve_binary_with(
            "coral-daemon",
            Some(custom_bin.as_os_str()),
            Some(&home),
            &[Path::new("/usr/bin")],
        );
        assert_eq!(resolved, expected.to_string_lossy());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discovers_default_alpine_rootfs() {
        let root = make_temp_dir("rootfs");
        let state = root.join("state");
        let rootfs = state.join("coral/images/alpine-3.20/rootfs");
        fs::create_dir_all(rootfs.join("bin")).unwrap();
        fs::write(rootfs.join("bin/sh"), b"").unwrap();

        assert_eq!(
            resolve_default_rootfs_in(Some(&state), None),
            Some(rootfs)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn merged_path_prepends_local_bin_without_duplication() {
        let root = make_temp_dir("merge-path");
        let home = root.join("home");
        let local_bin = home.join(".local/bin");
        fs::create_dir_all(&local_bin).unwrap();

        let path = std::env::join_paths([PathBuf::from("/usr/bin"), local_bin.clone()]).unwrap();
        let merged = merged_path_with_local_bin(Some(path.as_os_str()), Some(&home)).unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert_eq!(entries.first(), Some(&local_bin));
        assert_eq!(
            entries.iter().filter(|entry| **entry == local_bin).count(),
            1
        );
        let _ = fs::remove_dir_all(&root);
    }
}
