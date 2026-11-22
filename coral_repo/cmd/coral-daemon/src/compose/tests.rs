use std::fs;
use super::*;
use tempfile::tempdir;

#[test]
fn parses_compose_yaml_and_sorts_depends_on_topologically() {
    let yaml = r#"
name: coral-shop
services:
  web:
    image: alpine:3.20
    command: /bin/sh -c "echo web"
    depends_on:
      - api
      - redis
    ports:
      - "18080:8080"
    environment:
      API_URL: http://api:9000
      MODE: production
    volumes:
      - shop-assets:/var/www
  api:
    image: alpine:3.20
    depends_on:
      - redis
    environment:
      - REDIS_HOST=redis
    mem_limit: 128m
  redis:
    image: alpine:3.20
    command: ["/bin/sh", "-c", "echo redis"]
volumes:
  shop-assets:
"#;
    let spec = ComposeSpec::from_yaml_str(yaml, None).unwrap();
    assert_eq!(spec.project_name.as_deref(), Some("coral-shop"));
    let names: Vec<&str> = spec.services.iter().map(|s| s.name.as_str()).collect();
    // redis must come first, then api, then web!
    assert_eq!(names, vec!["redis", "api", "web"]);
    assert_eq!(spec.volumes, vec!["shop-assets"]);

    let web = &spec.services[2];
    assert_eq!(web.ports, vec!["18080:8080"]);
    assert!(web.envs.contains(&"API_URL=http://api:9000".to_string()));
    assert!(web.extra_hosts.contains(&"redis:127.0.0.1".to_string()));
    assert!(web.extra_hosts.contains(&"api:127.0.0.1".to_string()));
}

#[test]
fn parses_compose_restart_policy_and_healthcheck() {
    let yaml = r#"
services:
  worker:
    image: alpine:3.20
    restart: on-failure:3
    healthcheck:
      test: ["CMD-SHELL", "test -f /tmp/ready"]
      interval: 2s
      retries: 4
"#;
    let spec = ComposeSpec::from_yaml_str(yaml, None).unwrap();
    let w = &spec.services[0];
    assert_eq!(w.restart, "on-failure:3");
    assert_eq!(w.health_cmd.as_deref(), Some("test -f /tmp/ready"));
    assert_eq!(w.health_interval_ms, 2_000);
    assert_eq!(w.health_retries, 4);
}

#[test]
fn parses_full_compose_features_dotenv_envfile_entrypoint_target_conditions_profiles() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".env"), "APP_PORT=19090\nTAG=3.20\n").unwrap();
    fs::write(
        dir.path().join("app.env"),
        "# comment\nexport DB_NAME=prod_db\nOVERRIDE_ME=from_file\n",
    )
    .unwrap();
    let compose_path = dir.path().join("docker-compose.yml");
    fs::write(
        &compose_path,
        r#"
name: fullstack
networks:
  backend:
    driver: bridge
services:
  init-job:
    image: alpine:${TAG}
    entrypoint: ["/bin/sh", "-c"]
    command: ["echo migrated"]
  db:
    image: alpine:${TAG:-3.20}
    user: "1000:1000"
    healthcheck:
      test: ["CMD", "true"]
      interval: 1s
  app:
    build:
      context: .
      dockerfile: Dockerfile
      target: release
      args:
        APP_ENV: staging
    env_file:
      - app.env
    environment:
      OVERRIDE_ME: from_compose
      LITERAL_DOLLAR: $$escaped
    depends_on:
      init-job:
        condition: service_completed_successfully
      db:
        condition: service_healthy
    ports:
      - "${APP_PORT}:8080"
      - "5353:53/udp"
    labels:
      tier: backend
    networks:
      - backend
  debug-tool:
    image: alpine:3.20
    profiles: ["dev"]
"#,
    )
    .unwrap();

    let spec = ComposeSpec::from_file(&compose_path).unwrap();
    let app = spec.services.iter().find(|s| s.name == "app").unwrap();
    assert_eq!(app.build_target.as_deref(), Some("release"));
    assert_eq!(app.build_args, vec!["APP_ENV=staging"]);
    assert_eq!(app.ports, vec!["19090:8080", "5353:53/udp"]);
    assert_eq!(app.labels.get("tier").map(String::as_str), Some("backend"));
    assert!(app.envs.contains(&"DB_NAME=prod_db".to_string()));
    assert!(app.envs.contains(&"OVERRIDE_ME=from_compose".to_string()));
    assert!(app.envs.contains(&"LITERAL_DOLLAR=$escaped".to_string()));
    assert_eq!(
        app.depends_on_conditions,
        vec![
            ComposeDependency {
                service: "init-job".into(),
                condition: DependsCondition::ServiceCompletedSuccessfully,
            },
            ComposeDependency {
                service: "db".into(),
                condition: DependsCondition::ServiceHealthy,
            },
        ]
    );

    let init = spec.services.iter().find(|s| s.name == "init-job").unwrap();
    assert_eq!(init.entrypoint, vec!["/bin/sh", "-c"]);
    assert_eq!(init.command, vec!["echo migrated"]);

    // Profile filtering: default excludes `debug-tool`, `--profile dev` includes it.
    let default_sel: Vec<&str> = spec
        .select_services(&[], &[], false)
        .unwrap()
        .into_iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(!default_sel.contains(&"debug-tool"));
    assert!(default_sel.contains(&"app"));

    let dev_sel: Vec<&str> = spec
        .select_services(&[], &["dev".into()], false)
        .unwrap()
        .into_iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(dev_sel.contains(&"debug-tool"));
}

#[test]
fn yaml_with_dotenv_loads_sibling_env_for_rpc_compose() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".env"), "SVC_TAG=3.20\nHOST_PORT=18888\n").unwrap();
    let yaml = r#"
services:
  cache:
    image: alpine:${SVC_TAG}
    ports:
      - "${HOST_PORT}:6379"
"#;
    let spec = ComposeSpec::from_yaml_with_dotenv(yaml, Some(dir.path())).unwrap();
    assert_eq!(spec.services[0].image.as_deref(), Some("alpine:3.20"));
    assert_eq!(spec.services[0].ports, vec!["18888:6379"]);
}
