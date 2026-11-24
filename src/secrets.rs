//! Where tokens live: the system keychain (macOS Keychain, Windows Credential
//! Manager) and, when there is none (Linux without a native backend, CI,
//! `FDT_NO_KEYRING=1`), `~/.fdt/credentials.toml` with mode 0600.
//!
//! Also the redaction every message passes through: a `fdt_…` token never
//! reaches a log, an error or the terminal unless the user asked for it.

use anyhow::{Context, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const SERVICE: &str = "fdt";
const FILE: &str = "credentials.toml";

static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(fdt|fdr)_[A-Za-z0-9]+_[A-Za-z0-9_\-]+").unwrap());
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._\-]+").unwrap());
static URL_PASSWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-z][a-z0-9+.\-]*://[^:/@\s]+:)[^@\s]+@").unwrap());

/// Hides tokens, bearer values and passwords inside URLs.
pub fn redact(text: &str) -> String {
    let text = TOKEN.replace_all(text, "${1}_***");
    let text = BEARER.replace_all(&text, "${1}***");
    URL_PASSWORD.replace_all(&text, "${1}***@").into_owned()
}

/// What a masked secret looks like in output.
pub const MASK: &str = "••••••";

/// Writes a secret to a file only its owner can read (0600 on Unix).
pub fn write_private(path: &Path, text: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("no se pudo escribir {}", path.display()))?;
        file.write_all(text.as_bytes())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, text).with_context(|| format!("no se pudo escribir {}", path.display()))?;
    Ok(())
}

/// How a stored token is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    Keyring,
    File,
}

impl Store {
    pub fn label(self) -> &'static str {
        match self {
            Self::Keyring => "llavero del sistema",
            Self::File => "archivo 0600",
        }
    }
}

/// Whether this build and environment can use the system keychain.
pub fn keyring_available() -> bool {
    if std::env::var_os("FDT_NO_KEYRING").is_some_and(|v| !v.is_empty() && v != "0") {
        return false;
    }
    cfg!(any(target_os = "macos", target_os = "windows"))
}

fn file_path(home: &Path) -> PathBuf {
    home.join(FILE)
}

fn read_file(home: &Path) -> Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(file_path(home)) {
        Ok(raw) => toml::from_str(&raw).context("credentials.toml no es válido"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_file(home: &Path, map: &BTreeMap<String, String>) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let path = file_path(home);
    if map.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let raw = toml::to_string(map)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(raw.as_bytes())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, raw)?;
    Ok(())
}

/// Saves a token under `key`; returns where it went.
pub fn save(home: &Path, key: &str, token: &str) -> Result<Store> {
    if keyring_available() {
        let saved = keyring::Entry::new(SERVICE, key).and_then(|e| e.set_password(token));
        if saved.is_ok() {
            // A stale copy in the file would shadow nothing, but drop it anyway.
            let mut map = read_file(home)?;
            if map.remove(key).is_some() {
                write_file(home, &map)?;
            }
            return Ok(Store::Keyring);
        }
        crate::ui::warn("No se pudo usar el llavero del sistema; el token se guarda en un archivo 0600.");
    }
    let mut map = read_file(home)?;
    map.insert(key.to_owned(), token.to_owned());
    write_file(home, &map)?;
    Ok(Store::File)
}

/// Reads a token saved under `key` in `store`.
pub fn load(home: &Path, key: &str, store: Store) -> Result<Option<String>> {
    match store {
        Store::Keyring if keyring_available() => {
            match keyring::Entry::new(SERVICE, key).and_then(|e| e.get_password()) {
                Ok(token) => Ok(Some(token)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(anyhow::anyhow!("no se pudo leer el llavero del sistema: {e}")),
            }
        }
        Store::Keyring => Ok(None),
        Store::File => Ok(read_file(home)?.get(key).cloned()),
    }
}

/// Forgets a token wherever it is.
pub fn delete(home: &Path, key: &str) -> Result<()> {
    if keyring_available()
        && let Ok(entry) = keyring::Entry::new(SERVICE, key)
    {
        let _ = entry.delete_credential();
    }
    let mut map = read_file(home)?;
    if map.remove(key).is_some() {
        write_file(home, &map)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_tokens_bearers_and_url_passwords() {
        assert_eq!(redact("token fdt_ab12_SeCrEt-x"), "token fdt_***");
        assert_eq!(redact("Authorization: Bearer eyJhb.c.d"), "Authorization: Bearer ***");
        assert_eq!(
            redact("postgres://user:p4ss@db-1:5432/app"),
            "postgres://user:***@db-1:5432/app"
        );
        assert_eq!(redact("nada que ocultar"), "nada que ocultar");
    }

    #[test]
    fn file_store_is_private() {
        let home = tempfile::tempdir().unwrap();
        let mut map = BTreeMap::new();
        map.insert("k".to_owned(), "fdt_1_x".to_owned());
        write_file(home.path(), &map).unwrap();
        assert_eq!(read_file(home.path()).unwrap()["k"], "fdt_1_x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file_path(home.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
