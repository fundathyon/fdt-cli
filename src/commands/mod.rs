//! The commands. Each one resolves names to ids, calls Core and prints the
//! API's `data` as JSON, or a table when a person is reading.

pub mod db;
pub mod deploy;
pub mod env;
pub mod misc;
pub mod org;
pub mod project;
pub mod service;
pub mod session;

use crate::api::{Api, enc};
use crate::config::OrgRef;
use crate::context::Ctx;
use anyhow::Result;
use std::time::Duration;

/// `30s`, `10m`, `2h`, `1d` or plain seconds.
pub fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    let t = text.trim();
    let (number, unit) = match t.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => t.split_at(i),
        None => (t, "s"),
    };
    let n: u64 = number.parse().map_err(|_| format!("duración inválida: {text} (usa 30s, 10m, 2h o 1d)"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => return Err(format!("duración inválida: {text} (usa 30s, 10m, 2h o 1d)")),
    };
    if secs == 0 {
        return Err("la duración debe ser mayor que 0".into());
    }
    Ok(Duration::from_secs(secs))
}

/// `10m` (that long ago) or an RFC 3339 date.
pub fn parse_instant(text: &str) -> std::result::Result<chrono::DateTime<chrono::Utc>, String> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text.trim()) {
        return Ok(t.with_timezone(&chrono::Utc));
    }
    let d = parse_duration(text).map_err(|_| format!("«{text}» no es una duración (10m, 2h) ni una fecha RFC 3339"))?;
    Ok(chrono::Utc::now() - chrono::Duration::from_std(d).map_err(|e| e.to_string())?)
}

/// A client with the token and the organization in context.
pub async fn org_api(ctx: &Ctx) -> Result<(Api, OrgRef)> {
    let api = ctx.api()?;
    let org = ctx.org(&api).await?;
    Ok((api, org))
}

pub fn services_path(org: &str) -> String {
    format!("/organizations/{}/deploy/services", enc(org))
}

pub fn service_path(org: &str, service: &str) -> String {
    format!("{}/{}", services_path(org), enc(service))
}

pub fn databases_path(org: &str) -> String {
    format!("/organizations/{}/deploy/databases", enc(org))
}

/// How long one poll step waits: `FDT_POLL_UNIT_MS` (tests) or 1 s.
pub fn poll_unit() -> Duration {
    std::env::var("FDT_POLL_UNIT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Duration::from_secs(1), Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
        assert!(parse_duration("10x").is_err());
        assert!(parse_duration("0s").is_err());
        let t = parse_instant("2026-11-23T10:00:00Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-11-23T10:00:00+00:00");
        let ago = parse_instant("10m").unwrap();
        assert!((chrono::Utc::now() - ago).num_seconds() >= 599);
        assert!(parse_instant("ayer").is_err());
    }
}
