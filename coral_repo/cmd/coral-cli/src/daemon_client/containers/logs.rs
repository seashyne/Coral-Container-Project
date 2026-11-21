//! `coral logs` — show or stream logs for a `coral-daemon` container.

use crate::daemon_client::{connect_channel, container_state_label, format_nanos};
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, ContainerStatusRequest};
use std::io::Write;
use std::time::Duration;

fn slice_tail_lines(log: &str, tail: Option<usize>) -> &str {
    let Some(n) = tail else {
        return log;
    };
    if n == 0 {
        return "";
    }
    let trimmed_end = log.strip_suffix('\n').unwrap_or(log);
    let mut newline_positions: Vec<usize> =
        trimmed_end.match_indices('\n').map(|(i, _)| i).collect();
    if newline_positions.len() < n {
        return log;
    }
    let start = newline_positions.swap_remove(newline_positions.len() - n) + 1;
    &log[start..]
}

pub async fn cmd_logs(
    addr: &str,
    id: &str,
    show_status: bool,
    follow: bool,
    tail: Option<usize>,
) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let resp = rt
        .container_status(ContainerStatusRequest {
            container_id: id.to_string(),
            verbose: true,
        })
        .await
        .with_context(|| format!("fetch logs for container '{id}'"))?
        .into_inner();

    if show_status && let Some(st) = resp.status.as_ref() {
        let restart = resp
            .info
            .get("restartPolicy")
            .map(String::as_str)
            .unwrap_or("no");
        let restarts = resp
            .info
            .get("restartCount")
            .map(String::as_str)
            .unwrap_or("0");
        let health = resp
            .info
            .get("healthStatus")
            .map(String::as_str)
            .unwrap_or("none");
        println!(
            "state={} exit_code={} reason={} restart={}({}) health={} started={} finished={}",
            container_state_label(st.state),
            st.exit_code,
            if st.reason.is_empty() {
                "-"
            } else {
                &st.reason
            },
            restart,
            restarts,
            health,
            format_nanos(st.started_at),
            format_nanos(st.finished_at)
        );
    }

    let initial = resp.info.get("logOutput").map(String::as_str).unwrap_or("");
    let to_print = slice_tail_lines(initial, tail);
    if !to_print.is_empty() {
        print!("{to_print}");
        let _ = std::io::stdout().flush();
    } else if !show_status && !follow {
        eprintln!("(no log output yet for '{id}')");
    }

    if !follow {
        return Ok(());
    }

    let mut printed_bytes = initial.len();
    if resp
        .status
        .as_ref()
        .is_some_and(|st| st.state == ContainerState::ContainerExited as i32)
    {
        return Ok(());
    }

    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let poll = rt
            .container_status(ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: true,
            })
            .await
            .with_context(|| format!("stream logs for container '{id}'"))?
            .into_inner();
        let current = poll.info.get("logOutput").map(String::as_str).unwrap_or("");
        if current.len() > printed_bytes && current.is_char_boundary(printed_bytes) {
            let chunk = &current[printed_bytes..];
            print!("{chunk}");
            let _ = std::io::stdout().flush();
            printed_bytes = current.len();
        } else if current.len() < printed_bytes {
            printed_bytes = current.len();
        }
        if poll
            .status
            .as_ref()
            .is_some_and(|st| st.state == ContainerState::ContainerExited as i32)
        {
            break;
        }
    }
    Ok(())
}
