//! Docker-compatible `.dockerignore` parser, glob matcher (`*`, `**`, `?`, `!` negation),
//! and filtered build-context copier.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
struct IgnoreRule {
    negated: bool,
    pattern: String,
}

/// Compiled `.dockerignore` matcher for a build context directory.
#[derive(Debug, Default, Clone)]
pub struct DockerIgnore {
    rules: Vec<IgnoreRule>,
}

impl DockerIgnore {
    /// Loads `<ctx_dir>/.dockerignore` if present; returns an empty matcher otherwise.
    pub fn load(ctx_dir: &Path) -> Self {
        let path = ctx_dir.join(".dockerignore");
        match fs::read_to_string(&path) {
            Ok(raw) => Self::parse(&raw),
            Err(_) => Self::default(),
        }
    }

    /// Parses raw `.dockerignore` content into ordered rules.
    pub fn parse(raw: &str) -> Self {
        let mut rules = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let (negated, pat_raw) = if let Some(rest) = trimmed.strip_prefix('!') {
                (true, rest.trim())
            } else {
                (false, trimmed)
            };
            let norm = pat_raw.trim_start_matches("./").trim_matches('/');
            if !norm.is_empty() {
                rules.push(IgnoreRule {
                    negated,
                    pattern: norm.to_string(),
                });
            }
        }
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Returns `true` when `rel_path` (relative to build context root) is excluded.
    pub fn is_ignored(&self, rel_path: &Path) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        let rel = rel_str.trim_start_matches("./").trim_matches('/');
        if rel.is_empty() {
            return false;
        }

        let mut ignored = false;
        for rule in &self.rules {
            if rule_matches(&rule.pattern, rel) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

fn rule_matches(pattern: &str, rel: &str) -> bool {
    if glob_match(pattern, rel) {
        return true;
    }
    // Directory prefix match: pattern `node_modules` excludes `node_modules/foo/bar.js`.
    let mut prefix = String::new();
    for (idx, part) in rel.split('/').enumerate() {
        if idx > 0 {
            prefix.push('/');
        }
        prefix.push_str(part);
        if glob_match(pattern, &prefix) {
            return true;
        }
    }
    false
}

/// Glob matcher supporting `*` (single segment), `**` (any segments), and `?` (single char).
fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_rec(&p, &t)
}

fn glob_rec(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    if p.len() >= 2 && p[0] == '*' && p[1] == '*' {
        let mut next_p = &p[2..];
        if next_p.first() == Some(&'/') {
            next_p = &next_p[1..];
        }
        if glob_rec(next_p, t) {
            return true;
        }
        for i in 1..=t.len() {
            if glob_rec(next_p, &t[i..]) {
                return true;
            }
        }
        return false;
    }
    match p[0] {
        '*' => {
            if glob_rec(&p[1..], t) {
                return true;
            }
            if !t.is_empty() && t[0] != '/' {
                return glob_rec(p, &t[1..]);
            }
            false
        }
        '?' => !t.is_empty() && t[0] != '/' && glob_rec(&p[1..], &t[1..]),
        ch => !t.is_empty() && t[0] == ch && glob_rec(&p[1..], &t[1..]),
    }
}

/// Recursively copies `src` to `dst`, skipping any path excluded by `ignore` relative to `ctx_root`.
pub(super) fn copy_context_filtered(
    src: &Path,
    dst: &Path,
    force_dir: bool,
    ctx_root: &Path,
    ignore: &DockerIgnore,
) -> Result<()> {
    let meta = fs::symlink_metadata(src)
        .with_context(|| format!("stat copy source '{}'", src.display()))?;
    let rel = src.strip_prefix(ctx_root).unwrap_or(src);
    if !rel.as_os_str().is_empty() && ignore.is_ignored(rel) {
        return Ok(());
    }

    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let child_rel = child_src.strip_prefix(ctx_root).unwrap_or(&child_src);
            if ignore.is_ignored(child_rel) {
                continue;
            }
            let child_dst = dst.join(entry.file_name());
            copy_context_filtered(&child_src, &child_dst, false, ctx_root, ignore)?;
        }
        Ok(())
    } else if meta.file_type().is_symlink() {
        let link_target = fs::read_link(src)?;
        let target_file = resolve_target_file(src, dst, force_dir)?;
        let _ = fs::remove_file(&target_file);
        std::os::unix::fs::symlink(&link_target, &target_file)?;
        Ok(())
    } else {
        let target_file = resolve_target_file(src, dst, force_dir)?;
        fs::copy(src, &target_file)?;
        Ok(())
    }
}

fn resolve_target_file(src: &Path, dst: &Path, force_dir: bool) -> Result<std::path::PathBuf> {
    if force_dir || dst.is_dir() {
        fs::create_dir_all(dst)?;
        Ok(dst.join(
            src.file_name()
                .ok_or_else(|| anyhow!("invalid source filename"))?,
        ))
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(dst.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_dockerignore_globs_negation_and_filters_context_copy() {
        let ign = DockerIgnore::parse(
            "\
# comment
node_modules/
target
**/*.log
!important.log
*.tmp
",
        );
        assert!(ign.is_ignored(Path::new("node_modules/pkg/index.js")));
        assert!(ign.is_ignored(Path::new("target/debug/app")));
        assert!(ign.is_ignored(Path::new("logs/app.log")));
        assert!(!ign.is_ignored(Path::new("important.log")));
        assert!(ign.is_ignored(Path::new("foo.tmp")));
        assert!(!ign.is_ignored(Path::new("src/main.rs")));

        let ctx = tempdir().unwrap();
        let out = tempdir().unwrap();
        fs::create_dir_all(ctx.path().join("node_modules/a")).unwrap();
        fs::create_dir_all(ctx.path().join("src")).unwrap();
        fs::write(ctx.path().join("node_modules/a/big.js"), b"ignored").unwrap();
        fs::write(ctx.path().join("src/main.rs"), b"fn main() {}").unwrap();
        fs::write(ctx.path().join("important.log"), b"keep").unwrap();
        fs::write(ctx.path().join("scratch.tmp"), b"skip").unwrap();

        copy_context_filtered(ctx.path(), out.path(), true, ctx.path(), &ign).unwrap();
        assert!(out.path().join("src/main.rs").is_file());
        assert!(out.path().join("important.log").is_file());
        assert!(!out.path().join("node_modules").exists());
        assert!(!out.path().join("scratch.tmp").exists());
    }
}
