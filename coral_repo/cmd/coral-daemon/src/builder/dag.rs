//! BuildKit-style Multi-Stage DAG planner (topological parallel waves + dead-stage pruning)
//! and multi-architecture (`--platform`) predefined `ARG`s + QEMU `binfmt_misc` detection.

use super::dockerfile::{expand_vars, parse_copy_args, parse_from_instruction, split_shell_tokens, unquote};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A parsed Dockerfile stage node in the build dependency graph.
#[derive(Debug, Clone)]
pub(super) struct StageNode {
    pub index: usize,
    pub name: Option<String>,
    pub deps: Vec<usize>,
    pub instructions: Vec<(String, String)>,
    pub step_offset: usize,
}

/// Returns the BuildKit predefined platform `ARG` map (`BUILDOS`, `BUILDARCH`, `BUILDPLATFORM`,
/// `TARGETOS`, `TARGETARCH`, `TARGETVARIANT`, `TARGETPLATFORM`).
pub fn buildkit_platform_args(platform_override: Option<&str>) -> HashMap<String, String> {
    let build_os = "linux".to_string();
    let build_arch = host_oci_arch().to_string();
    let build_platform = format!("{build_os}/{build_arch}");

    let (target_os, target_arch, target_variant) = match platform_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => parse_platform_triple(raw, &build_os, &build_arch),
        None => (build_os.clone(), build_arch.clone(), String::new()),
    };
    let target_platform = if target_variant.is_empty() {
        format!("{target_os}/{target_arch}")
    } else {
        format!("{target_os}/{target_arch}/{target_variant}")
    };

    let mut map = HashMap::new();
    map.insert("BUILDOS".into(), build_os);
    map.insert("BUILDARCH".into(), build_arch);
    map.insert("BUILDPLATFORM".into(), build_platform);
    map.insert("TARGETOS".into(), target_os);
    map.insert("TARGETARCH".into(), target_arch);
    map.insert("TARGETVARIANT".into(), target_variant);
    map.insert("TARGETPLATFORM".into(), target_platform);
    map
}

/// Returns the OCI architecture identifier for the current host CPU.
pub fn host_oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "arm",
        "riscv64" => "riscv64",
        other => other,
    }
}

/// Parses a Docker/OCI `--platform` string (`linux/arm64`, `linux/arm/v7`, `arm64`, etc.).
pub fn parse_platform_triple(raw: &str, default_os: &str, default_arch: &str) -> (String, String, String) {
    let parts: Vec<&str> = raw.split('/').map(str::trim).filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [arch] => (default_os.to_string(), normalize_arch(arch), String::new()),
        [os, arch] => (os.to_ascii_lowercase(), normalize_arch(arch), String::new()),
        [os, arch, variant, ..] => (
            os.to_ascii_lowercase(),
            normalize_arch(arch),
            variant.to_ascii_lowercase(),
        ),
        _ => (default_os.to_string(), default_arch.to_string(), String::new()),
    }
}

fn normalize_arch(arch: &str) -> String {
    match arch.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        "armv7" | "armv7l" | "arm" => "arm".into(),
        "i386" | "i686" | "386" => "386".into(),
        other => other.to_string(),
    }
}

/// Checks whether QEMU `binfmt_misc` emulation is registered for cross-arch execution.
pub fn check_cross_arch_qemu(target_arch: &str) -> Result<(), String> {
    check_cross_arch_qemu_in(
        Path::new("/proc/sys/fs/binfmt_misc"),
        host_oci_arch(),
        target_arch,
    )
}

/// Testable implementation of `check_cross_arch_qemu` against a given `binfmt_misc` directory.
pub fn check_cross_arch_qemu_in(
    binfmt_dir: &Path,
    host_arch: &str,
    target_arch: &str,
) -> Result<(), String> {
    let target = normalize_arch(target_arch);
    let host = normalize_arch(host_arch);
    if target == host {
        return Ok(());
    }
    let qemu_name = match target.as_str() {
        "arm64" => "qemu-aarch64",
        "amd64" => "qemu-x86_64",
        "arm" => "qemu-arm",
        "riscv64" => "qemu-riscv64",
        "386" => "qemu-i386",
        other => return Err(format!("unsupported cross-arch target '{other}'")),
    };
    if binfmt_dir.join(qemu_name).exists() {
        Ok(())
    } else {
        Err(format!(
            "cross-arch target '{target}' on host '{host}' requires {qemu_name} in {} \
             (install `qemu-user-static` / `binfmt-support` or `tonistiigi/binfmt`)",
            binfmt_dir.display()
        ))
    }
}

/// Parsed multi-stage breakdown produced by [`split_into_stages`].
pub(super) type SplitStages = (HashMap<String, String>, Vec<StageNode>, HashMap<String, usize>);

/// Splits parsed Dockerfile instructions into pre-`FROM` global `ARG`s and a list of [`StageNode`]s
/// with their dependency edges (`FROM <stage>` and `COPY --from=<stage|index>`).
pub(super) fn split_into_stages(
    instructions: &[(String, String)],
    initial_args: &HashMap<String, String>,
) -> Result<SplitStages> {
    let mut global_args = initial_args.clone();
    let mut stages: Vec<StageNode> = Vec::new();
    let mut stage_names: HashMap<String, usize> = HashMap::new();

    for (idx, (verb, raw_arg)) in instructions.iter().enumerate() {
        if stages.is_empty() {
            if verb == "ARG" {
                let expanded = expand_vars(raw_arg, &global_args);
                for tok in split_shell_tokens(&expanded) {
                    if let Some((k, v)) = tok.split_once('=') {
                        let key = k.trim().to_string();
                        global_args.entry(key).or_insert_with(|| unquote(v.trim()));
                    } else if !tok.trim().is_empty() {
                        global_args.entry(tok.trim().to_string()).or_default();
                    }
                }
                continue;
            }
            if verb != "FROM" {
                bail!("{verb} cannot appear before FROM");
            }
        }

        if verb == "FROM" {
            let expanded = expand_vars(raw_arg, &global_args);
            let from_info = parse_from_instruction(&expanded)?;
            let stage_idx = stages.len();
            let mut deps = Vec::new();
            if let Some(&dep_idx) = stage_names.get(&from_info.base_ref.to_ascii_lowercase()) {
                deps.push(dep_idx);
            }
            if let Some(ref sname) = from_info.stage_name {
                stage_names.insert(sname.to_ascii_lowercase(), stage_idx);
            }
            stages.push(StageNode {
                index: stage_idx,
                name: from_info.stage_name,
                deps,
                instructions: vec![(verb.clone(), raw_arg.clone())],
                step_offset: idx,
            });
        } else {
            let cur_idx = stages.len() - 1;
            if verb == "COPY" || verb == "ADD" {
                let expanded = expand_vars(raw_arg, &global_args);
                let (from_ref, _) = parse_copy_args(&expanded);
                if let Some(fref) = from_ref {
                    let dep = stage_names
                        .get(&fref.to_ascii_lowercase())
                        .copied()
                        .or_else(|| fref.parse::<usize>().ok().filter(|&i| i < cur_idx));
                    if let Some(dep_idx) = dep
                        && dep_idx < cur_idx
                        && !stages[cur_idx].deps.contains(&dep_idx)
                    {
                        stages[cur_idx].deps.push(dep_idx);
                    }
                }
            }
            stages[cur_idx]
                .instructions
                .push((verb.clone(), raw_arg.clone()));
        }
    }

    if stages.is_empty() {
        bail!("Dockerfile is missing a FROM instruction");
    }
    Ok((global_args, stages, stage_names))
}

/// Computes topological parallel execution waves and prunes unreachable stages
/// (BuildKit dead-stage elimination) for the requested `target_stage` (or the final stage).
pub(super) fn plan_stage_waves(
    stages: &[StageNode],
    target_stage: Option<&str>,
) -> Result<(Vec<Vec<usize>>, usize)> {
    let target_idx = if let Some(wanted) = target_stage
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        stages
            .iter()
            .position(|s| {
                s.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
            })
            .ok_or_else(|| anyhow::anyhow!("target stage '{wanted}' not found in Dockerfile"))?
    } else {
        stages.len() - 1
    };

    // Walk backward from target_idx to keep only reachable stages.
    let mut reachable: HashSet<usize> = HashSet::new();
    let mut stack = vec![target_idx];
    while let Some(id) = stack.pop() {
        if reachable.insert(id) {
            for &dep in &stages[id].deps {
                stack.push(dep);
            }
        }
    }

    // Group reachable stages into topological parallel waves.
    let mut scheduled: HashSet<usize> = HashSet::new();
    let mut waves: Vec<Vec<usize>> = Vec::new();
    while scheduled.len() < reachable.len() {
        let mut wave: Vec<usize> = reachable
            .iter()
            .copied()
            .filter(|id| {
                !scheduled.contains(id)
                    && stages[*id].deps.iter().all(|d| scheduled.contains(d))
            })
            .collect();
        wave.sort_unstable();
        if wave.is_empty() {
            bail!("cyclic stage dependency detected in Dockerfile");
        }
        for &id in &wave {
            scheduled.insert(id);
        }
        waves.push(wave);
    }

    Ok((waves, target_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::dockerfile::parse_dockerfile;
    use tempfile::tempdir;

    #[test]
    fn plans_parallel_multistage_waves_and_prunes_dead_stages() {
        let df = "\
FROM scratch AS fe
RUN echo fe
FROM scratch AS be
RUN echo be
FROM scratch AS unused_debug
RUN echo unused
FROM scratch AS final
COPY --from=fe /a /a
COPY --from=be /b /b
";
        let instrs = parse_dockerfile(df).unwrap();
        let (_, stages, _) = split_into_stages(&instrs, &HashMap::new()).unwrap();
        assert_eq!(stages.len(), 4);

        let (waves, target_idx) = plan_stage_waves(&stages, None).unwrap();
        assert_eq!(target_idx, 3);
        // Wave 0 runs `fe` (0) and `be` (1) in parallel; `unused_debug` (2) is pruned!
        assert_eq!(waves, vec![vec![0, 1], vec![3]]);

        let (fe_waves, fe_target) = plan_stage_waves(&stages, Some("fe")).unwrap();
        assert_eq!(fe_target, 0);
        assert_eq!(fe_waves, vec![vec![0]]);
    }

    #[test]
    fn builds_platform_predefined_args_and_detects_qemu_binfmt() {
        let args = buildkit_platform_args(Some("linux/arm64"));
        assert_eq!(args.get("TARGETOS").map(String::as_str), Some("linux"));
        assert_eq!(args.get("TARGETARCH").map(String::as_str), Some("arm64"));
        assert_eq!(
            args.get("TARGETPLATFORM").map(String::as_str),
            Some("linux/arm64")
        );
        assert_eq!(args.get("BUILDOS").map(String::as_str), Some("linux"));

        let fake_binfmt = tempdir().unwrap();
        assert!(check_cross_arch_qemu_in(fake_binfmt.path(), "amd64", "amd64").is_ok());
        assert!(check_cross_arch_qemu_in(fake_binfmt.path(), "amd64", "arm64").is_err());
        std::fs::write(fake_binfmt.path().join("qemu-aarch64"), b"enabled\n").unwrap();
        assert!(check_cross_arch_qemu_in(fake_binfmt.path(), "amd64", "arm64").is_ok());
    }
}
