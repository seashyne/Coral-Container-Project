use super::dockerfile::{parse_copy_args, parse_dockerfile, parse_from_instruction};

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
