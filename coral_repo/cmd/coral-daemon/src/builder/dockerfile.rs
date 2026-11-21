//! Dockerfile text parsing: instructions, flags, and shell-word splitting.

use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;

pub(super) struct FromInstruction {
    pub(super) base_ref: String,
    pub(super) stage_name: Option<String>,
}

pub(super) fn parse_from_instruction(arg: &str) -> Result<FromInstruction> {
    let tokens = split_shell_tokens(arg);
    let mut non_flags = tokens
        .into_iter()
        .filter(|t| !t.starts_with("--"))
        .collect::<Vec<_>>();
    if non_flags.is_empty() {
        bail!("FROM instruction requires an image reference");
    }
    let base_ref = non_flags.remove(0);
    let mut stage_name = None;
    if non_flags.len() >= 2 && non_flags[0].eq_ignore_ascii_case("AS") {
        stage_name = Some(non_flags[1].clone());
    }
    Ok(FromInstruction {
        base_ref,
        stage_name,
    })
}

pub(super) fn parse_copy_args(arg: &str) -> (Option<String>, Vec<String>) {
    let tokens = split_shell_tokens(arg);
    let mut from_ref = None;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        if let Some(val) = tok.strip_prefix("--from=") {
            if !val.trim().is_empty() {
                from_ref = Some(val.trim().to_string());
            }
        } else if tok == "--from" && i + 1 < tokens.len() {
            i += 1;
            from_ref = Some(tokens[i].trim().to_string());
        } else if !tok.starts_with("--") {
            paths.push(tok.clone());
        }
        i += 1;
    }
    (from_ref, paths)
}

pub(super) fn split_shell_tokens(s: &str) -> Vec<String> {
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

pub(super) fn parse_exec_or_shell_instruction(arg: &str) -> Vec<String> {
    let trimmed = arg.trim();
    if trimmed.starts_with('[')
        && let Ok(parsed) = serde_json::from_str::<Vec<String>>(trimmed)
    {
        return parsed;
    }
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec!["/bin/sh".into(), "-c".into(), trimmed.to_string()]
    }
}

pub(super) fn parse_dockerfile(raw: &str) -> Result<Vec<(String, String)>> {
    let mut joined_lines = Vec::new();
    let mut current = String::new();

    for line in raw.lines() {
        let trimmed_start = line.trim_start();
        if current.is_empty() && (trimmed_start.is_empty() || trimmed_start.starts_with('#')) {
            continue;
        }
        if trimmed_start.starts_with('#') {
            continue;
        }
        let trimmed_end = line.trim_end();
        if let Some(without_slash) = trimmed_end.strip_suffix('\\') {
            current.push_str(without_slash.trim());
            current.push(' ');
        } else {
            current.push_str(line.trim());
            let full = std::mem::take(&mut current);
            if !full.is_empty() {
                joined_lines.push(full);
            }
        }
    }
    if !current.trim().is_empty() {
        joined_lines.push(current.trim().to_string());
    }

    let mut out = Vec::new();
    for line in joined_lines {
        let (verb, rest) = line
            .split_once(char::is_whitespace)
            .ok_or_else(|| anyhow!("malformed Dockerfile line: '{line}'"))?;
        out.push((verb.trim().to_ascii_uppercase(), rest.trim().to_string()));
    }
    Ok(out)
}

pub(super) fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut out = input.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
        out = out.replace(&format!("${k}"), v);
    }
    out
}

pub(super) fn unquote(s: &str) -> String {
    if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
        && s.len() >= 2
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

pub(super) fn set_env_var(envs: &mut Vec<String>, key: &str, val: &str) {
    let prefix = format!("{key}=");
    envs.retain(|e| !e.starts_with(&prefix));
    envs.push(format!("{key}={val}"));
}
