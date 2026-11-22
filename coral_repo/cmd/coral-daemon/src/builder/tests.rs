use super::buildkit::{
    RunMountKind, apply_metadata_instruction, parse_run_mounts,
};
use super::dockerfile::{
    apply_env_instruction, expand_vars, parse_copy_args, parse_dockerfile, parse_from_instruction,
    sync_stage_vars_from_envs,
};
use crate::images::OciImageConfig;
use std::collections::HashMap;

#[test]
fn parses_multiline_dockerfile_with_comments() {
    let df = r#"
            # Base image
            FROM alpine:3.20
            ARG APP_ENV=prod
            ENV MODE=$APP_ENV
            WORKDIR /app
            RUN echo hello \
                && echo world
            COPY . /app
        "#;
    let parsed = parse_dockerfile(df).unwrap();
    assert_eq!(parsed.len(), 6);
    assert_eq!(parsed[0], ("FROM".into(), "alpine:3.20".into()));
    assert_eq!(parsed[4].0, "RUN");
    assert!(parsed[4].1.contains("echo hello && echo world"));
}

#[test]
fn parses_multistage_from_and_copy_flags() {
    let f1 = parse_from_instruction("--platform=linux/amd64 alpine:3.20 AS builder").unwrap();
    assert_eq!(f1.base_ref, "alpine:3.20");
    assert_eq!(f1.stage_name.as_deref(), Some("builder"));

    let f2 = parse_from_instruction("scratch as final").unwrap();
    assert_eq!(f2.base_ref, "scratch");
    assert_eq!(f2.stage_name.as_deref(), Some("final"));

    let (from_ref, paths) = parse_copy_args("--from=builder --chown=1000:1000 /out/bin /app/bin");
    assert_eq!(from_ref.as_deref(), Some("builder"));
    assert_eq!(paths, vec!["/out/bin", "/app/bin"]);

    let (from_idx, paths2) = parse_copy_args("--from 0 /a /b");
    assert_eq!(from_idx.as_deref(), Some("0"));
    assert_eq!(paths2, vec!["/a", "/b"]);
}

#[test]
fn expand_vars_avoids_prefix_collision_and_recursive_expansion() {
    let mut vars = HashMap::new();
    vars.insert("APP".into(), "/opt/app".into());
    vars.insert("APP_PORT".into(), "8080".into());
    vars.insert("DOLLAR".into(), "$APP".into());

    assert_eq!(expand_vars("$APP:$APP_PORT:${APP}/bin", &vars), "/opt/app:8080:/opt/app/bin");
    // Unbound variables and prefix-similar unbound names stay intact.
    assert_eq!(expand_vars("$APP_EXTRA $RUNTIME_VAR", &vars), "$APP_EXTRA $RUNTIME_VAR");
    // Escaped `\$` and `$` inside replaced values are not re-expanded.
    assert_eq!(expand_vars(r"\$APP $DOLLAR", &vars), r"\$APP $APP");
}

#[test]
fn env_instruction_expands_stage_path_and_supports_multipair_and_legacy() {
    let mut envs = vec![
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "TERM=xterm".to_string(),
    ];
    let mut vars = HashMap::new();
    sync_stage_vars_from_envs(&envs, &mut vars);

    let expanded = expand_vars("PATH=\"$PATH:/app/bin\"", &vars);
    apply_env_instruction(&expanded, &mut vars, &mut envs);
    assert!(envs.contains(&"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/app/bin".to_string()));

    apply_env_instruction("A=1 B=\"two words\"", &mut vars, &mut envs);
    assert!(envs.contains(&"A=1".to_string()));
    assert!(envs.contains(&"B=two words".to_string()));

    apply_env_instruction("GREETING hello world", &mut vars, &mut envs);
    assert!(envs.contains(&"GREETING=hello world".to_string()));
}

#[test]
fn parses_buildkit_heredoc_run_mounts_and_metadata_instructions() {
    let df = r#"
FROM alpine:3.20
EXPOSE 80 53/udp
VOLUME ["/data", "/var/log"]
SHELL ["/bin/sh", "-cex"]
HEALTHCHECK --interval=2s --retries=4 CMD test -f /tmp/healthy
ONBUILD RUN echo child-hook
RUN --mount=type=cache,id=apk,target=/var/cache/apk --mount=type=secret,id=api_key,src=secret.txt <<EOF
echo step1
echo step2
EOF
"#;
    let parsed = parse_dockerfile(df).unwrap();
    let mut cfg = OciImageConfig::default();
    for (verb, arg) in &parsed {
        apply_metadata_instruction(verb, arg, &mut cfg);
    }
    assert_eq!(cfg.exposed_ports, vec!["80/tcp", "53/udp"]);
    assert_eq!(cfg.volumes, vec!["/data", "/var/log"]);
    assert_eq!(cfg.shell, vec!["/bin/sh", "-cex"]);
    assert_eq!(cfg.healthcheck_cmd.as_deref(), Some("test -f /tmp/healthy"));
    assert_eq!(cfg.healthcheck_interval_ms, 2_000);
    assert_eq!(cfg.healthcheck_retries, 4);
    assert_eq!(cfg.onbuild, vec!["RUN echo child-hook"]);

    let run_line = &parsed.last().unwrap().1;
    let (mounts, cmd) = parse_run_mounts(run_line).unwrap();
    assert_eq!(
        mounts,
        vec![
            RunMountKind::Cache {
                id: "apk".into(),
                target: "/var/cache/apk".into()
            },
            RunMountKind::Secret {
                id: "api_key".into(),
                target: "/run/secrets/api_key".into(),
                src: Some("secret.txt".into())
            }
        ]
    );
    assert_eq!(cmd, "echo step1 && echo step2");
}
