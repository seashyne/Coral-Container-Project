//! `.env` parsing and `${VAR:-default}` interpolation for Compose files.

use super::text::{strip_yaml_comment, unquote_yaml};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub(super) fn upsert_env_kv(envs: &mut Vec<String>, key: &str, val: &str) {
    if key.is_empty() {
        return;
    }
    let prefix = format!("{key}=");
    envs.retain(|e| !e.starts_with(&prefix));
    envs.push(format!("{key}={val}"));
}

pub(super) fn parse_dotenv_file(path: &Path) -> Result<Vec<(String, String)>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read env file '{}'", path.display()))?;
    let mut pairs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let clean = trimmed.strip_prefix("export ").map(str::trim).unwrap_or(trimmed);
        if let Some((k, v)) = clean.split_once('=') {
            let key = k.trim();
            if !key.is_empty() {
                let raw_val = v.trim();
                let val = if (raw_val.starts_with('"') && raw_val.ends_with('"'))
                    || (raw_val.starts_with('\'') && raw_val.ends_with('\''))
                {
                    unquote_yaml(raw_val)
                } else {
                    strip_yaml_comment(raw_val).trim().to_string()
                };
                pairs.push((key.to_string(), val));
            }
        }
    }
    Ok(pairs)
}

pub fn expand_compose_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if i + 1 < chars.len() && chars[i + 1] == '$' {
            out.push('$');
            i += 2;
            continue;
        }
        if i + 1 < chars.len()
            && chars[i + 1] == '{'
            && let Some(rel_end) = chars[i + 2..].iter().position(|&c| c == '}')
        {
            let expr: String = chars[i + 2..i + 2 + rel_end].iter().collect();
            let val = eval_brace_var(&expr, vars);
            out.push_str(&val);
            i += 2 + rel_end + 1;
            continue;
        }
        if i + 1 < chars.len() && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_') {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let name: String = chars[start..end].iter().collect();
            if let Some(v) = vars.get(&name) {
                out.push_str(v);
            } else {
                // Preserve unknown bare `$VAR` (e.g. shell variables inside `command: /bin/sh -c "echo $VAR"`).
                out.push('$');
                out.push_str(&name);
            }
            i = end;
            continue;
        }
        out.push('$');
        i += 1;
    }

    out
}

pub(super) fn eval_brace_var(expr: &str, vars: &HashMap<String, String>) -> String {
    if let Some((name, default)) = expr.split_once(":-") {
        match vars.get(name.trim()) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => default.to_string(),
        }
    } else if let Some((name, default)) = expr.split_once('-') {
        match vars.get(name.trim()) {
            Some(v) => v.clone(),
            None => default.to_string(),
        }
    } else {
        vars.get(expr.trim()).cloned().unwrap_or_default()
    }
}
