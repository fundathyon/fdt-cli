//! `fdt update`: replaces this binary with the latest GitHub Release, after
//! checking it against the release's SHA256SUMS. Same mechanism as keel.

use crate::failure::{Kind, fail};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// GitHub repo whose Releases carry the binaries. `FDT_RELEASE_REPO` overrides it.
pub const RELEASE_REPO: &str = "fundathyon/fdt-cli";

fn repo() -> String {
    std::env::var("FDT_RELEASE_REPO").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| RELEASE_REPO.to_owned())
}

pub fn asset_for(os: &str, arch: &str) -> Result<String> {
    let triple = match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => {
            return Err(fail(
                Kind::Error,
                format!("No hay binario publicado para {os}/{arch}; compílalo con `cargo install --path .`."),
            ));
        }
    };
    Ok(if os == "windows" { format!("fdt-{triple}.exe") } else { format!("fdt-{triple}") })
}

/// `v1.2.3` → (1, 2, 3, is_prerelease).
fn parse(tag: &str) -> Option<(u64, u64, u64, bool)> {
    let t = tag.trim().trim_start_matches('v');
    let (core, pre) = match t.split_once('-') {
        Some((c, _)) => (c, true),
        None => (t, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??, pre))
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => (l.0, l.1, l.2, !l.3) > (c.0, c.1, c.2, !c.3),
        (Some(_), None) => true,
        _ => false,
    }
}

pub fn expected_sha256<'a>(sums: &'a str, asset: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset).then_some(hash)
    })
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(crate::api::user_agent()).build()?)
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| fail(Kind::Network, format!("No se pudo descargar {url}: {e}")))?;
    if !response.status().is_success() {
        return Err(fail(Kind::Network, format!("GitHub respondió {} al descargar {url}", response.status().as_u16())));
    }
    Ok(response.bytes().await?.to_vec())
}

pub async fn latest_release_tag() -> Result<String> {
    let repo = repo();
    let response = http()?
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| fail(Kind::Network, format!("No se pudo consultar GitHub: {e}")))?;
    if !response.status().is_success() {
        return Err(fail(
            Kind::Network,
            format!(
                "GitHub respondió {} al buscar la última versión de fdt en {repo} (¿aún no hay releases?).",
                response.status().as_u16()
            ),
        ));
    }
    let body: serde_json::Value = response.json().await?;
    let tag = body["tag_name"].as_str().unwrap_or_default().to_owned();
    if parse(&tag).is_none() {
        return Err(fail(Kind::Error, format!("La última release de fdt no tiene un tag válido: {tag}")));
    }
    Ok(tag)
}

pub enum Status {
    UpToDate,
    Available,
    Updated,
}

pub async fn update(current: &str, check: bool) -> Result<(Status, String, String)> {
    let current = format!("v{}", current.trim_start_matches('v'));
    let latest = latest_release_tag().await?;
    if !is_newer(&latest, &current) {
        return Ok((Status::UpToDate, current, latest));
    }
    if check {
        return Ok((Status::Available, current, latest));
    }
    let asset = asset_for(std::env::consts::OS, std::env::consts::ARCH)?;
    let base = format!("https://github.com/{}/releases/download/{latest}", repo());
    let client = http()?;
    let sums = String::from_utf8_lossy(&download(&client, &format!("{base}/SHA256SUMS")).await?).into_owned();
    let expected = expected_sha256(&sums, &asset)
        .ok_or_else(|| fail(Kind::Error, format!("La release {latest} no trae la suma de {asset}; no se instala.")))?
        .to_owned();
    let binary = download(&client, &format!("{base}/{asset}")).await?;
    let actual = format!("{:x}", Sha256::digest(&binary));
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(fail(Kind::Error, format!("La suma de {asset} no coincide con SHA256SUMS; no se instala.")));
    }
    let dir = std::env::temp_dir().join(format!("fdt-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&asset);
    std::fs::write(&path, &binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    self_replace::self_replace(&path)
        .with_context(|| format!("no se pudo reemplazar el binario actual; descárgalo de {base}/{asset}"))?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok((Status::Updated, current, latest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_assets_and_compares_versions() {
        assert_eq!(asset_for("macos", "aarch64").unwrap(), "fdt-aarch64-apple-darwin");
        assert_eq!(asset_for("windows", "x86_64").unwrap(), "fdt-x86_64-pc-windows-msvc.exe");
        assert!(asset_for("freebsd", "x86_64").is_err());
        assert!(is_newer("v0.2.0", "v0.1.9"));
        assert!(!is_newer("v0.1.0", "v0.1.0"));
        assert!(!is_newer("v0.2.0-rc.1", "v0.2.0"));
        assert!(is_newer("v0.2.0", "v0.2.0-rc.1"));
        let sums = "abc  fdt-x86_64-apple-darwin\n";
        assert_eq!(expected_sha256(sums, "fdt-x86_64-apple-darwin"), Some("abc"));
    }
}
