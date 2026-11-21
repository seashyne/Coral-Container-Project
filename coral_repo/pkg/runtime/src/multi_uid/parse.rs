//! Parsing and construction of subordinate UID/GID ranges and OCI ID mappings.

use super::SubIdRange;
use coral_oci::LinuxIdMapping;

pub(super) const MAX_CONTAINER_SUBIDS: u32 = 65_535;
const MAX_IDMAP_EXTENTS: usize = 16;

/// Parses subordinate ID ranges matching `username` or numeric `uid` from `/etc/subuid` or `/etc/subgid`.
pub(super) fn parse_subid_ranges(
    content: &str,
    username: Option<&str>,
    uid: u32,
) -> Vec<SubIdRange> {
    let uid_str = uid.to_string();
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split(':');
        let (Some(owner), Some(start_s), Some(count_s)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let owner = owner.trim();
        let owner_matches = owner == uid_str || username.is_some_and(|u| owner == u);
        if !owner_matches {
            continue;
        }
        let Ok(start) = start_s.trim().parse::<u32>() else {
            continue;
        };
        let Ok(count) = count_s.trim().parse::<u32>() else {
            continue;
        };
        if start == 0 || count == 0 {
            continue;
        }
        out.push(SubIdRange { start, count });
    }
    out
}

/// Constructs OCI ID mappings placing `0 -> host_id (1)` followed by subordinate ranges for `1..`.
pub(super) fn build_subid_mappings(
    host_id: u32,
    ranges: &[SubIdRange],
    max_subids: u32,
) -> Vec<LinuxIdMapping> {
    let mut mappings = vec![LinuxIdMapping {
        container_id: 0,
        host_id,
        size: 1,
    }];
    let mut next_container_id: u32 = 1;
    let mut remaining = max_subids;

    for range in ranges.iter().take(MAX_IDMAP_EXTENTS) {
        if remaining == 0 {
            break;
        }
        let end = range.start.saturating_add(range.count);
        // Avoid accidental overlap with container ID 0's host_id.
        if host_id >= range.start && host_id < end {
            continue;
        }
        let take = range
            .count
            .min(remaining)
            .min(u32::MAX.saturating_sub(next_container_id));
        if take == 0 {
            continue;
        }
        mappings.push(LinuxIdMapping {
            container_id: next_container_id,
            host_id: range.start,
            size: take,
        });
        next_container_id = next_container_id.saturating_add(take);
        remaining = remaining.saturating_sub(take);
    }

    mappings
}

/// Formats `LinuxIdMapping` entries as command-line arguments (`[cid, hid, size, ...]`) for
/// `newuidmap` / `newgidmap`.
pub(super) fn format_helper_args(pid: i32, mappings: &[LinuxIdMapping]) -> Vec<String> {
    let mut args = Vec::with_capacity(1 + mappings.len() * 3);
    args.push(pid.to_string());
    for m in mappings {
        args.push(m.container_id.to_string());
        args.push(m.host_id.to_string());
        args.push(m.size.to_string());
    }
    args
}
