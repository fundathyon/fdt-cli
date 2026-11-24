//! `fdt update`: replaces this binary with the latest GitHub Release, after
//! checking the downloaded archive against the release's SHA256SUMS.
//!
//! Release layout (built by `.github/workflows/release.yml`, also read by
//! `install.sh` and `install.ps1`):
//!
//! ```text
//! <releases>/download/vX.Y.Z/fdt-<target>.tar.gz      # Linux and macOS: `fdt` at the archive root
//! <releases>/download/vX.Y.Z/fdt-<target>.zip         # Windows: `fdt.exe` at the archive root
//! <releases>/download/vX.Y.Z/fdt-<target>.<ext>.sha256
//! <releases>/download/vX.Y.Z/SHA256SUMS               # `sha256sum` format, one line per archive
//! ```
//!
//! `<releases>` is `https://github.com/fundathyon/fdt-cli/releases`; the
//! latest tag comes from where `<releases>/latest` redirects, which unlike
//! the GitHub API has no per-IP rate limit (a hackathon behind one NAT).

use crate::failure::{Kind, fail};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;

/// GitHub repo whose Releases carry the binaries. `FDT_RELEASE_REPO` overrides it.
pub const RELEASE_REPO: &str = "fundathyon/fdt-cli";

fn repo() -> String {
    std::env::var("FDT_RELEASE_REPO").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| RELEASE_REPO.to_owned())
}

/// Base URL of the releases, e.g. `https://github.com/fundathyon/fdt-cli/releases`.
/// `FDT_RELEASES_URL` overrides it (a mirror, or a local server in tests).
pub fn releases_url() -> String {
    match std::env::var("FDT_RELEASES_URL").ok().filter(|u| !u.is_empty()) {
        Some(url) => url.trim_end_matches('/').to_owned(),
        None => format!("https://github.com/{}/releases", repo()),
    }
}

/// The Rust target a release archive is built for.
pub fn target_for(os: &str, arch: &str) -> Result<&'static str> {
    Ok(match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => {
            return Err(fail(
                Kind::Error,
                format!("No hay binario publicado para {os}/{arch}; compílalo con `cargo install --path .`."),
            ));
        }
    })
}

/// The release archive for this OS and architecture, e.g. `fdt-aarch64-apple-darwin.tar.gz`.
pub fn asset_for(os: &str, arch: &str) -> Result<String> {
    let target = target_for(os, arch)?;
    Ok(if os == "windows" { format!("fdt-{target}.zip") } else { format!("fdt-{target}.tar.gz") })
}

/// `v1.2.3` → (1, 2, 3, is_prerelease).
fn parse(tag: &str) -> Option<(u64, u64, u64, bool)> {
    let t = tag.trim().trim_start_matches('v');
    let (core, pre) = match t.split_once('-') {
        Some((c, _)) => (c, true),
        None => (t, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??, pre);
    parts.next().is_none().then_some(version)
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => (l.0, l.1, l.2, !l.3) > (c.0, c.1, c.2, !c.3),
        (Some(_), None) => true,
        _ => false,
    }
}

/// The sha256 for `asset` in a `sha256sum`-style file.
pub fn expected_sha256<'a>(sums: &'a str, asset: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset).then_some(hash)
    })
}

/// The tag in the `Location` of `<releases>/latest`: `…/releases/tag/v0.1.0` → `v0.1.0`.
fn tag_from_location(location: &str) -> Option<String> {
    let (_, tag) = location.trim_end_matches('/').rsplit_once("/tag/")?;
    let tag = tag.split(['?', '#']).next()?;
    parse(tag).map(|_| tag.to_owned())
}

/// Pulls the `fdt` (or `fdt.exe`) binary out of a release archive.
pub fn extract_binary(asset: &str, archive: &[u8]) -> Result<Vec<u8>> {
    let is_binary = |path: &str| {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        name == "fdt" || name == "fdt.exe"
    };
    let mut binary = Vec::new();
    if asset.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).context("el .zip de la release no es válido")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_file() && is_binary(entry.name()) {
                entry.read_to_end(&mut binary)?;
                return Ok(binary);
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        for entry in tar.entries().context("el .tar.gz de la release no es válido")? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().into_owned();
            if entry.header().entry_type().is_file() && is_binary(&path) {
                entry.read_to_end(&mut binary)?;
                return Ok(binary);
            }
        }
    }
    Err(fail(Kind::Error, format!("{asset} no contiene el binario de fdt; no se instala.")))
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

/// The latest stable release tag, from the redirect of `<releases>/latest`;
/// falls back to the GitHub API if that redirect ever stops carrying the tag.
pub async fn latest_release_tag(releases: &str) -> Result<String> {
    let no_redirects =
        reqwest::Client::builder().user_agent(crate::api::user_agent()).redirect(reqwest::redirect::Policy::none()).build()?;
    let url = format!("{releases}/latest");
    let response = no_redirects
        .get(&url)
        .send()
        .await
        .map_err(|e| fail(Kind::Network, format!("No se pudo consultar {url}: {e}")))?;
    if response.status().is_redirection() {
        let location = response.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()).unwrap_or_default();
        if let Some(tag) = tag_from_location(location) {
            return Ok(tag);
        }
        // GitHub sends /releases/latest to /releases when there is no stable release yet.
        if location.trim_end_matches('/').ends_with("/releases") {
            return Err(fail(Kind::Error, format!("Aún no hay ninguna versión publicada de fdt en {releases}.")));
        }
    }
    if std::env::var_os("FDT_RELEASES_URL").is_some() {
        return Err(fail(
            Kind::Network,
            format!("{url} respondió {} sin redirigir a una versión.", response.status().as_u16()),
        ));
    }
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

/// Downloads `asset` of release `tag`, checks it against SHA256SUMS and
/// returns the binary inside it. Nothing touches the disk.
pub async fn download_verified(releases: &str, tag: &str, asset: &str) -> Result<Vec<u8>> {
    let base = format!("{releases}/download/{tag}");
    let client = http()?;
    let sums = String::from_utf8_lossy(&download(&client, &format!("{base}/SHA256SUMS")).await?).into_owned();
    let expected = expected_sha256(&sums, asset)
        .ok_or_else(|| fail(Kind::Error, format!("La release {tag} no trae la suma de {asset}; no se instala.")))?
        .to_owned();
    let archive = download(&client, &format!("{base}/{asset}")).await?;
    let actual = format!("{:x}", Sha256::digest(&archive));
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(fail(Kind::Error, format!("La suma de {asset} no coincide con SHA256SUMS; no se instala.")));
    }
    extract_binary(asset, &archive)
}

pub enum Status {
    UpToDate,
    Available,
    Updated,
}

pub async fn update(current: &str, check: bool) -> Result<(Status, String, String)> {
    let current = format!("v{}", current.trim_start_matches('v'));
    let releases = releases_url();
    let latest = latest_release_tag(&releases).await?;
    if !is_newer(&latest, &current) {
        return Ok((Status::UpToDate, current, latest));
    }
    if check {
        return Ok((Status::Available, current, latest));
    }
    let asset = asset_for(std::env::consts::OS, std::env::consts::ARCH)?;
    let binary = download_verified(&releases, &latest, &asset).await?;
    let dir = std::env::temp_dir().join(format!("fdt-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(if cfg!(windows) { "fdt.exe" } else { "fdt" });
    std::fs::write(&path, &binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    let replaced = self_replace::self_replace(&path).with_context(|| {
        format!(
            "no se pudo reemplazar {} (¿permisos?); descarga {releases}/download/{latest}/{asset} o reinstala con install.sh / install.ps1",
            std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
        )
    });
    let _ = std::fs::remove_dir_all(&dir);
    replaced?;
    Ok((Status::Updated, current, latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn tar_gz(name: &str, data: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
        let mut license = tar::Header::new_gnu();
        license.set_size(3);
        license.set_mode(0o644);
        license.set_cksum();
        builder.append_data(&mut license, "LICENSE", &b"MIT"[..]).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn zip_with(name: &str, data: &[u8]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("LICENSE", options).unwrap();
        zip.write_all(b"MIT").unwrap();
        zip.start_file(name, options).unwrap();
        zip.write_all(data).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn picks_assets_and_compares_versions() {
        assert_eq!(asset_for("macos", "aarch64").unwrap(), "fdt-aarch64-apple-darwin.tar.gz");
        assert_eq!(asset_for("macos", "x86_64").unwrap(), "fdt-x86_64-apple-darwin.tar.gz");
        assert_eq!(asset_for("linux", "x86_64").unwrap(), "fdt-x86_64-unknown-linux-musl.tar.gz");
        assert_eq!(asset_for("linux", "aarch64").unwrap(), "fdt-aarch64-unknown-linux-musl.tar.gz");
        assert_eq!(asset_for("windows", "x86_64").unwrap(), "fdt-x86_64-pc-windows-msvc.zip");
        assert!(asset_for("windows", "aarch64").is_err());
        assert!(asset_for("freebsd", "x86_64").is_err());
        assert!(is_newer("v0.2.0", "v0.1.9"));
        assert!(!is_newer("v0.1.0", "v0.1.0"));
        assert!(!is_newer("v0.2.0-rc.1", "v0.2.0"));
        assert!(is_newer("v0.2.0", "v0.2.0-rc.1"));
        assert!(parse("v1.2.3.4").is_none());
        let sums = "abc  fdt-x86_64-apple-darwin.tar.gz\ndef *fdt-x86_64-pc-windows-msvc.zip\n";
        assert_eq!(expected_sha256(sums, "fdt-x86_64-apple-darwin.tar.gz"), Some("abc"));
        assert_eq!(expected_sha256(sums, "fdt-x86_64-pc-windows-msvc.zip"), Some("def"));
        assert_eq!(expected_sha256(sums, "fdt-aarch64-apple-darwin.tar.gz"), None);
    }

    #[test]
    fn reads_the_tag_from_the_latest_redirect() {
        let at = "https://github.com/fundathyon/fdt-cli/releases/tag/v0.1.0";
        assert_eq!(tag_from_location(at).as_deref(), Some("v0.1.0"));
        assert_eq!(tag_from_location("/releases/tag/v1.2.3-rc.1").as_deref(), Some("v1.2.3-rc.1"));
        assert_eq!(tag_from_location("https://github.com/fundathyon/fdt-cli/releases"), None);
        assert_eq!(tag_from_location("https://github.com/x/y/releases/tag/nightly"), None);
    }

    #[test]
    fn extracts_the_binary_from_both_archive_kinds() {
        let tgz = tar_gz("fdt", b"unix-binary");
        assert_eq!(extract_binary("fdt-x86_64-apple-darwin.tar.gz", &tgz).unwrap(), b"unix-binary");
        let zip = zip_with("fdt.exe", b"windows-binary");
        assert_eq!(extract_binary("fdt-x86_64-pc-windows-msvc.zip", &zip).unwrap(), b"windows-binary");
        let other = tar_gz("README.md", b"x");
        assert!(extract_binary("fdt-x86_64-apple-darwin.tar.gz", &other).is_err());
        assert!(extract_binary("fdt-x86_64-pc-windows-msvc.zip", b"not a zip").is_err());
    }

    #[tokio::test]
    async fn finds_the_latest_tag_and_verifies_the_download() {
        let server = MockServer::start().await;
        let releases = format!("{}/releases", server.uri());
        let asset = "fdt-x86_64-unknown-linux-musl.tar.gz";
        let archive = tar_gz("fdt", b"new-fdt");
        let good = format!("{:x}  {asset}\n", Sha256::digest(&archive));
        Mock::given(method("GET"))
            .and(path("/releases/latest"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", format!("{releases}/tag/v0.2.0")))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/releases/download/v0.2.0/{asset}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/releases/download/v0.2.0/SHA256SUMS"))
            .respond_with(ResponseTemplate::new(200).set_body_string(good))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/releases/download/v0.3.0/SHA256SUMS"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!("{}  {asset}\n", "0".repeat(64))))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/releases/download/v0.3.0/{asset}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive))
            .mount(&server)
            .await;

        assert_eq!(latest_release_tag(&releases).await.unwrap(), "v0.2.0");
        assert_eq!(download_verified(&releases, "v0.2.0", asset).await.unwrap(), b"new-fdt");
        let tampered = download_verified(&releases, "v0.3.0", asset).await.unwrap_err();
        assert!(tampered.to_string().contains("no coincide"), "{tampered}");
        let missing = download_verified(&releases, "v0.2.0", "fdt-aarch64-unknown-linux-musl.tar.gz").await.unwrap_err();
        assert!(missing.to_string().contains("no trae la suma"), "{missing}");
    }
}
