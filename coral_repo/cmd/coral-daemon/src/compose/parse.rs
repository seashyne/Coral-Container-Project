//! Loading Compose files from disk and selecting services from a parsed stack.

use super::model::{ComposeService, ComposeSpec};
use super::util::parse_dotenv_file;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

impl ComposeSpec {
    /// Loads and parses a `docker-compose.yml` / `coral-compose.yml` file from disk,
    /// automatically loading sibling `.env` variables for `${VAR:-default}` interpolation.
    pub fn from_file(path: &Path) -> Result<Self> {
        Self::from_file_with_env(path, None)
    }

    /// Loads a Compose file with optional explicit `--env-file` overriding `.env`.
    pub fn from_file_with_env(path: &Path, extra_env_file: Option<&Path>) -> Result<Self> {
        let content = fs::read_to_string(path)
            .map_err(|e| anyhow!("read compose file '{}': {e}", path.display()))?;
        let base_dir = path.parent();
        let interpolation_vars = load_interpolation_vars(base_dir, extra_env_file, true)?;
        Self::from_yaml_with_env(&content, base_dir, &interpolation_vars)
    }

    /// Parses a Compose YAML string and loads sibling `.env` from `base_dir` if present.
    pub fn from_yaml_with_dotenv(raw: &str, base_dir: Option<&Path>) -> Result<Self> {
        let interpolation_vars = load_interpolation_vars(base_dir, None, false)?;
        Self::from_yaml_with_env(raw, base_dir, &interpolation_vars)
    }

    /// Parses a Docker Compose / Coral Compose YAML string without external env overrides.
    pub fn from_yaml_str(raw: &str, base_dir: Option<&Path>) -> Result<Self> {
        Self::from_yaml_with_env(raw, base_dir, &HashMap::new())
    }


    /// Selects services matching `active_profiles` and optional target service names (automatically
    /// including transitive `depends_on` dependencies unless `no_deps == true`), in topological order.
    pub fn select_services<'a>(
        &'a self,
        requested: &[String],
        active_profiles: &[String],
        no_deps: bool,
    ) -> Result<Vec<&'a ComposeService>> {
        let mut wanted: HashSet<String> = HashSet::new();
        let by_name: HashMap<&str, &ComposeService> = self
            .services
            .iter()
            .map(|s| (s.name.as_str(), s))
            .collect();

        if requested.is_empty() {
            for svc in &self.services {
                let profile_ok = svc.profiles.is_empty()
                    || svc
                        .profiles
                        .iter()
                        .any(|p| active_profiles.iter().any(|ap| ap == p));
                if profile_ok {
                    wanted.insert(svc.name.clone());
                }
            }
        } else {
            let mut stack = requested.to_vec();
            while let Some(name) = stack.pop() {
                if !by_name.contains_key(name.as_str()) {
                    bail!("compose service '{name}' not found in compose file");
                }
                if wanted.insert(name.clone()) && !no_deps {
                    for dep in &by_name[name.as_str()].depends_on {
                        stack.push(dep.clone());
                    }
                }
            }
        }

        Ok(self
            .services
            .iter()
            .filter(|s| wanted.contains(&s.name))
            .collect())
    }
}

fn load_interpolation_vars(
    base_dir: Option<&Path>,
    extra_env_file: Option<&Path>,
    include_process_env: bool,
) -> Result<HashMap<String, String>> {
    let mut vars = HashMap::new();
    if let Some(dir) = base_dir {
        let sibling_env = dir.join(".env");
        if sibling_env.is_file() {
            for (k, v) in parse_dotenv_file(&sibling_env)? {
                vars.insert(k, v);
            }
        }
    }
    if let Some(ext) = extra_env_file {
        for (k, v) in parse_dotenv_file(ext)
            .with_context(|| format!("load --env-file '{}'", ext.display()))?
        {
            vars.insert(k, v);
        }
    }
    if include_process_env {
        for (k, v) in std::env::vars() {
            vars.insert(k, v);
        }
    }
    Ok(vars)
}
