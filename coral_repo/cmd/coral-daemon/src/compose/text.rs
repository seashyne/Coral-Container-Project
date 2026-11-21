//! Low-level YAML text helpers used by the Compose parser.

use anyhow::Result;
use std::fs;
use std::path::Path;

pub(super) fn resolve_maybe_rel(raw: &str, base_dir: Option<&Path>) -> String {
    if let Some(base) = base_dir
        && (raw == "."
            || raw.starts_with("./")
            || raw.starts_with("../")
            || (!raw.starts_with('/') && raw.contains('.')))
        && (raw.starts_with('.') || base.join(raw).exists())
    {
        let joined = base.join(raw);
        return fs::canonicalize(&joined)
            .unwrap_or(joined)
            .to_string_lossy()
            .into_owned();
    }
    raw.to_string()
}

pub(super) fn resolve_volume_entry(raw: &str, base_dir: Option<&Path>) -> String {
    let Some((src, rest)) = raw.split_once(':') else {
        return raw.to_string();
    };
    let s = src.trim();
    if s == "." || s.starts_with("./") || s.starts_with("../") {
        let resolved_src = resolve_maybe_rel(s, base_dir);
        format!("{resolved_src}:{rest}")
    } else {
        raw.to_string()
    }
}

pub(super) fn parse_command_value(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return parse_inline_list(trimmed);
    }
    let unquoted = unquote_yaml(trimmed);
    split_shell_words(&unquoted)
}

pub(super) fn parse_inline_list(raw: &str) -> Vec<String> {
    let inner = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    inner
        .split(',')
        .map(|s| unquote_yaml(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

pub(super) fn split_shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ws if ws.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            other => cur.push(other),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub(super) fn parse_health_test(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let items = parse_inline_list(trimmed);
        if items.first().is_some_and(|s| s.eq_ignore_ascii_case("CMD-SHELL")) {
            return items[1..].join(" ");
        }
        if items.first().is_some_and(|s| s.eq_ignore_ascii_case("CMD")) {
            return items[1..].join(" ");
        }
        return items.join(" ");
    }
    unquote_yaml(trimmed)
}

pub(super) fn parse_duration_ms(raw: &str) -> Option<u64> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(v) = lower.strip_suffix("ms") {
        return v.trim().parse::<u64>().ok();
    }
    if let Some(v) = lower.strip_suffix('s') {
        return v.trim().parse::<u64>().ok().map(|s| s.saturating_mul(1_000));
    }
    if let Some(v) = lower.strip_suffix('m') {
        return v.trim().parse::<u64>().ok().map(|m| m.saturating_mul(60_000));
    }
    lower.parse::<u64>().ok().map(|s| s.saturating_mul(1_000))
}

pub(super) fn parse_memory_mb(raw: &str) -> Result<i64> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(num) = lower.strip_suffix("gb").or_else(|| lower.strip_suffix('g')) {
        return Ok(num.trim().parse::<i64>()? * 1024);
    }
    if let Some(num) = lower.strip_suffix("mb").or_else(|| lower.strip_suffix('m')) {
        return Ok(num.trim().parse::<i64>()?);
    }
    Ok(lower.parse::<i64>()?)
}

pub(super) fn parse_cpu_percent(raw: &str) -> Result<i64> {
    let trimmed = raw.trim().trim_end_matches('%');
    if trimmed.contains('.') {
        let f: f64 = trimmed.parse()?;
        Ok((f * 100.0).round() as i64)
    } else {
        let n: i64 = trimmed.parse()?;
        Ok(if n <= 16 { n * 100 } else { n })
    }
}

pub(super) fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return &line[..i];
            }
            _ => {}
        }
    }
    line
}

pub(super) fn unquote_yaml(s: &str) -> String {
    let t = s.trim();
    if ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
        && t.len() >= 2
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}
