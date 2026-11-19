//! OCI image reference parser (`Docker Hub`, `Red Hat Registry`, `GHCR`, `scratch`)
//! and `WWW-Authenticate: Bearer` challenge parser.

use anyhow::{Result, bail};
use std::collections::HashMap;

/// Parsed OCI image reference (or the special empty `scratch` base image).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRef {
    Scratch,
    Oci {
        registry: String,
        repository: String,
        reference: String,
        display_tag: String,
    },
}

impl ImageRef {
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            bail!("image reference must not be empty");
        }
        if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
            bail!("image reference '{trimmed}' contains invalid whitespace or control characters");
        }
        if trimmed == "scratch" || trimmed == "scratch:latest" {
            return Ok(Self::Scratch);
        }

        // Split optional @digest or :tag (only after the last '/').
        let (name_part, reference, has_explicit_ref) =
            if let Some((name, digest)) = trimmed.split_once('@') {
                if name.is_empty() || digest.is_empty() {
                    bail!("invalid digest image reference '{trimmed}'");
                }
                (name, digest.to_string(), true)
            } else {
                let last_slash = trimmed.rfind('/');
                let last_colon = trimmed.rfind(':');
                match (last_slash, last_colon) {
                    (Some(s), Some(c)) if c > s => {
                        let tag = &trimmed[c + 1..];
                        if tag.is_empty() {
                            bail!("empty tag in image reference '{trimmed}'");
                        }
                        (&trimmed[..c], tag.to_string(), true)
                    }
                    (None, Some(c)) => {
                        let tag = &trimmed[c + 1..];
                        if tag.is_empty() {
                            bail!("empty tag in image reference '{trimmed}'");
                        }
                        (&trimmed[..c], tag.to_string(), true)
                    }
                    _ => (trimmed, "latest".to_string(), false),
                }
            };

        if name_part.is_empty() || name_part.contains("..") {
            bail!("invalid image name '{trimmed}'");
        }

        let (raw_registry, raw_repo) = match name_part.split_once('/') {
            Some((first, rest))
                if first.contains('.') || first.contains(':') || first == "localhost" =>
            {
                if rest.is_empty() {
                    bail!("missing repository in image reference '{trimmed}'");
                }
                (first, rest)
            }
            _ => ("registry-1.docker.io", name_part),
        };

        let registry = match raw_registry {
            "docker.io" | "index.docker.io" | "registry-1.docker.io" => {
                "registry-1.docker.io".to_string()
            }
            other => other.to_string(),
        };

        let repository = if registry == "registry-1.docker.io" && !raw_repo.contains('/') {
            format!("library/{raw_repo}")
        } else {
            raw_repo.to_string()
        };

        let display_tag = if has_explicit_ref {
            trimmed.to_string()
        } else {
            format!("{trimmed}:latest")
        };

        Ok(Self::Oci {
            registry,
            repository,
            reference,
            display_tag,
        })
    }
}

/// Canonicalizes an image lookup key so `"alpine"` matches `"alpine:latest"` and `"scratch"` matches `"scratch:latest"`.
pub fn normalize_lookup_tag(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed == "scratch" || trimmed == "scratch:latest" {
        return "scratch".to_string();
    }
    match ImageRef::parse(trimmed) {
        Ok(ImageRef::Oci { display_tag, .. }) => display_tag,
        _ => trimmed.to_string(),
    }
}

pub(crate) fn parse_bearer_challenge(header: &str) -> Option<HashMap<String, String>> {
    let rest = header
        .trim()
        .strip_prefix("Bearer ")
        .or_else(|| header.trim().strip_prefix("bearer "))?;
    let mut params = HashMap::new();
    let mut chars = rest.chars().peekable();

    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                break;
            }
            key.push(c);
            chars.next();
        }
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            break;
        }
        let mut val = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(escaped) = chars.next() {
                        val.push(escaped);
                    }
                } else if c == '"' {
                    break;
                } else {
                    val.push(c);
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' {
                    break;
                }
                val.push(c);
                chars.next();
            }
        }
        params.insert(key, val);
    }

    params.contains_key("realm").then_some(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_linux_base_image_refs() {
        assert_eq!(ImageRef::parse("scratch").unwrap(), ImageRef::Scratch);
        assert_eq!(ImageRef::parse("scratch:latest").unwrap(), ImageRef::Scratch);

        assert_eq!(
            ImageRef::parse("alpine").unwrap(),
            ImageRef::Oci {
                registry: "registry-1.docker.io".into(),
                repository: "library/alpine".into(),
                reference: "latest".into(),
                display_tag: "alpine:latest".into(),
            }
        );
        assert_eq!(
            ImageRef::parse("debian:bookworm-slim").unwrap(),
            ImageRef::Oci {
                registry: "registry-1.docker.io".into(),
                repository: "library/debian".into(),
                reference: "bookworm-slim".into(),
                display_tag: "debian:bookworm-slim".into(),
            }
        );
        assert_eq!(
            ImageRef::parse("ubuntu:24.04").unwrap(),
            ImageRef::Oci {
                registry: "registry-1.docker.io".into(),
                repository: "library/ubuntu".into(),
                reference: "24.04".into(),
                display_tag: "ubuntu:24.04".into(),
            }
        );
        assert_eq!(
            ImageRef::parse("rockylinux:9-minimal").unwrap(),
            ImageRef::Oci {
                registry: "registry-1.docker.io".into(),
                repository: "library/rockylinux".into(),
                reference: "9-minimal".into(),
                display_tag: "rockylinux:9-minimal".into(),
            }
        );
        assert_eq!(
            ImageRef::parse("registry.access.redhat.com/ubi9/ubi-minimal:latest").unwrap(),
            ImageRef::Oci {
                registry: "registry.access.redhat.com".into(),
                repository: "ubi9/ubi-minimal".into(),
                reference: "latest".into(),
                display_tag: "registry.access.redhat.com/ubi9/ubi-minimal:latest".into(),
            }
        );
        assert!(ImageRef::parse("").is_err());
        assert!(ImageRef::parse("../evil").is_err());
    }

    #[test]
    fn parses_bearer_www_authenticate_header() {
        let h = r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/debian:pull""#;
        let parsed = parse_bearer_challenge(h).unwrap();
        assert_eq!(parsed.get("realm").unwrap(), "https://auth.docker.io/token");
        assert_eq!(parsed.get("service").unwrap(), "registry.docker.io");
        assert_eq!(
            parsed.get("scope").unwrap(),
            "repository:library/debian:pull"
        );
    }
}
