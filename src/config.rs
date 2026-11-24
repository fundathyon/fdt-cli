//! Local state: `~/.fdt/config.toml` (active organization, API URL and which
//! tokens are saved where, never the tokens themselves) and the `.fdt.toml`
//! a repo carries after `fdt link`.

use crate::failure::{Kind, fail};
use crate::secrets::Store;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const LINK_FILE: &str = ".fdt.toml";
const CONFIG_FILE: &str = "config.toml";
/// Credential scope of a session token (`fdt login`), valid in every org.
pub const DEFAULT_SCOPE: &str = "default";

/// `FDT_HOME`, or `~/.fdt`.
pub fn home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FDT_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let base = directories::BaseDirs::new().context("no se encontró el directorio de tu usuario")?;
    Ok(base.home_dir().join(".fdt"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgRef {
    pub id: String,
    pub name: String,
}

/// What `config.toml` remembers of a saved token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub store: Store,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// API URL saved by `fdt login --api-url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// Active organization (`fdt use`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<OrgRef>,
    /// `<api_url>#<default|org id>` → where its token is.
    #[serde(default)]
    pub credentials: BTreeMap<String, Credential>,
}

pub fn credential_key(api_url: &str, scope: &str) -> String {
    format!("{api_url}#{scope}")
}

impl Config {
    pub fn load(home: &Path) -> Result<Self> {
        let path = home.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(raw) => toml::from_str(&raw)
                .with_context(|| format!("{} no es una configuración válida", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        std::fs::create_dir_all(home)
            .with_context(|| format!("no se pudo crear {}", home.display()))?;
        let path = home.join(CONFIG_FILE);
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

/// Accepts `api.acme.dev/core/api/v1` and defaults the scheme to https.
pub fn normalize_api_url(input: &str) -> Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    let without_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    if without_scheme.is_empty() || trimmed.ends_with(':') {
        return Err(fail(Kind::Usage, format!("URL de API inválida: {input}")));
    }
    let lower = trimmed.to_ascii_lowercase();
    let with_scheme = if lower.starts_with("http://") || lower.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .map_err(|_| fail(Kind::Usage, format!("URL de API inválida: {input}")))?;
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(fail(Kind::Usage, format!("URL de API inválida: {input}")));
    }
    Ok(with_scheme)
}

/// `.fdt.toml`: the org, project and service a repo works with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

/// The nearest `.fdt.toml` from `start` upwards.
pub fn find_link(start: &Path) -> Result<Option<(PathBuf, Link)>> {
    for dir in start.ancestors() {
        let path = dir.join(LINK_FILE);
        if path.is_file() {
            let raw = std::fs::read_to_string(&path)?;
            let link: Link = toml::from_str(&raw)
                .with_context(|| format!("{} no es válido", path.display()))?;
            return Ok(Some((path, link)));
        }
    }
    Ok(None)
}

/// Where `fdt link` writes: the root of the git repo, or the current folder.
pub fn link_target(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd)
        .join(LINK_FILE)
}

pub fn write_link(path: &Path, link: &Link) -> Result<()> {
    let body = format!(
        "# Contexto de fdt para este repo (fdt link / fdt unlink). No contiene secretos.\n{}",
        toml::to_string_pretty(link)?
    );
    std::fs::write(path, body).with_context(|| format!("no se pudo escribir {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_without_tokens() {
        let home = tempfile::tempdir().unwrap();
        let mut config = Config {
            org: Some(OrgRef { id: "o1".into(), name: "Acme".into() }),
            ..Config::default()
        };
        config.credentials.insert(
            credential_key("https://x", DEFAULT_SCOPE),
            Credential {
                store: Store::File,
                token_id: Some("t1".into()),
                kind: Some("session".into()),
                user: None,
                organization_id: None,
                expires_at: None,
            },
        );
        config.save(home.path()).unwrap();
        let loaded = Config::load(home.path()).unwrap();
        assert_eq!(loaded.org, config.org);
        assert_eq!(loaded.credentials, config.credentials);
    }

    #[test]
    fn finds_the_link_upwards() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(find_link(&nested).unwrap().is_none());
        let link = Link { org: Some("o1".into()), project: Some("p1".into()), ..Link::default() };
        write_link(&dir.path().join(LINK_FILE), &link).unwrap();
        let (path, found) = find_link(&nested).unwrap().unwrap();
        assert_eq!(found, link);
        assert_eq!(path, dir.path().join(LINK_FILE));
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(
            normalize_api_url("api.acme.dev/core/api/v1/").unwrap(),
            "https://api.acme.dev/core/api/v1"
        );
        assert_eq!(normalize_api_url("http://127.0.0.1:8010").unwrap(), "http://127.0.0.1:8010");
        assert!(normalize_api_url("https://").is_err());
    }
}
