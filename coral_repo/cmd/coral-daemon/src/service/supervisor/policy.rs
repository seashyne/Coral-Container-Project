//! Parsed Docker-compatible restart policy.

/// Parsed Docker-compatible restart policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPolicy {
    No,
    Always,
    UnlessStopped,
    OnFailure(Option<u32>),
}

impl RestartPolicy {
    pub fn parse(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        if lower == "always" {
            Self::Always
        } else if lower == "unless-stopped" {
            Self::UnlessStopped
        } else if let Some(rest) = lower.strip_prefix("on-failure") {
            let max = rest
                .strip_prefix(':')
                .and_then(|n| n.trim().parse::<u32>().ok())
                .filter(|&n| n > 0);
            Self::OnFailure(max)
        } else {
            Self::No
        }
    }

    pub fn should_restart(&self, exit_code: i32, restart_count: u32, stopped_by_user: bool) -> bool {
        if stopped_by_user {
            return false;
        }
        match self {
            Self::No => false,
            Self::Always | Self::UnlessStopped => true,
            Self::OnFailure(None) => exit_code != 0,
            Self::OnFailure(Some(max)) => exit_code != 0 && restart_count < *max,
        }
    }
}
