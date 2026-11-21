//! Human-readable CLI value parsers (`4G`, `512m`, `128`) and `--pull` policy helpers.

use clap::ValueEnum;

/// Image pull policy for `coral run` (matches Docker/Podman `--pull`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PullPolicy {
    /// Pull image from registry only if not already present locally (default).
    Missing,
    /// Always pull image before creating the container.
    Always,
    /// Never pull; fail immediately if the image is not installed locally.
    Never,
}

/// Parses memory strings like `0`, `128`, `128m`, `4G`, `1.5GiB` into MiB.
pub fn parse_memory_mb(raw: &str) -> Result<i64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("memory value must not be empty".into());
    }
    if s == "0" {
        return Ok(0);
    }

    let split_idx = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (num_part, unit_part) = s.split_at(split_idx);
    if num_part.is_empty() {
        return Err(format!("invalid memory value '{raw}'"));
    }

    let val: f64 = num_part
        .parse()
        .map_err(|_| format!("invalid memory number '{num_part}' in '{raw}'"))?;
    if !val.is_finite() || val < 0.0 {
        return Err(format!("memory must be non-negative: '{raw}'"));
    }
    if val == 0.0 {
        return Ok(0);
    }

    let unit = unit_part.trim().to_ascii_lowercase();
    let mb = match unit.as_str() {
        "" | "m" | "mb" | "mib" => val,
        "k" | "kb" | "kib" => (val / 1024.0).ceil().max(1.0),
        "g" | "gb" | "gib" => val * 1024.0,
        "t" | "tb" | "tib" => val * 1024.0 * 1024.0,
        other => {
            return Err(format!(
                "unsupported memory unit '{other}' in '{raw}' (use k, m, g, or t)"
            ));
        }
    };

    if mb > i64::MAX as f64 {
        return Err(format!("memory limit too large: '{raw}'"));
    }
    Ok(mb.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::parse_memory_mb;

    #[test]
    fn parses_plain_and_unit_memory_strings() {
        assert_eq!(parse_memory_mb("0").unwrap(), 0);
        assert_eq!(parse_memory_mb("128").unwrap(), 128);
        assert_eq!(parse_memory_mb("64m").unwrap(), 64);
        assert_eq!(parse_memory_mb("512MB").unwrap(), 512);
        assert_eq!(parse_memory_mb("4G").unwrap(), 4096);
        assert_eq!(parse_memory_mb("1.5gib").unwrap(), 1536);
        assert_eq!(parse_memory_mb("2048k").unwrap(), 2);
        assert_eq!(parse_memory_mb("512k").unwrap(), 1);
        assert_eq!(parse_memory_mb("1t").unwrap(), 1048576);
        assert!(parse_memory_mb("-5").is_err());
        assert!(parse_memory_mb("4x").is_err());
        assert!(parse_memory_mb("").is_err());
    }
}
