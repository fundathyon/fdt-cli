//! `fdt org list|show|usage|members`.

use crate::api::{Api, enc};
use crate::config::OrgRef;
use crate::context::{Ctx, org_ref};
use crate::output::{fields, s, when};
use crate::{resolve, ui};
use anyhow::Result;
use serde_json::Value;

async fn target(ctx: &Ctx, api: &Api, query: Option<String>) -> Result<OrgRef> {
    match query {
        Some(q) => Ok(org_ref(&resolve::org(api, &q).await?)),
        None => ctx.org(api).await,
    }
}

pub async fn list(ctx: &Ctx) -> Result<()> {
    let api = ctx.api()?;
    let orgs = Value::Array(resolve::orgs(&api).await?);
    let active = ctx.config.org.as_ref().map(|o| o.id.clone());
    ctx.out.print(&orgs, || {
        let rows: Vec<Vec<String>> = orgs
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                let mark = if active.as_deref() == o["id"].as_str() { "*" } else { " " };
                let roles = o["roles"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default();
                vec![format!("{mark} {}", s(o, "name")), roles, s(o, "id")]
            })
            .collect();
        ui::table(&["organización", "rol", "id"], &rows)
    })
}

pub async fn show(ctx: &Ctx, query: Option<String>) -> Result<()> {
    let api = ctx.api()?;
    let org = target(ctx, &api, query).await?;
    let data = api.get(&format!("/organizations/{}", enc(&org.id))).await?;
    ctx.out.print(&data, || {
        fields(&[
            ("nombre", s(&data, "name")),
            ("id", s(&data, "id")),
            ("descripción", s(&data, "description")),
            ("tu rol", s(&data, "my_role")),
            ("estado", s(&data, "status")),
            ("creada", when(&data, "created_at")),
        ])
    })
}

/// `{limit_key, used, limit}` objects anywhere in the usage, as rows.
fn limit_rows(prefix: &str, value: &Value, rows: &mut Vec<Vec<String>>) {
    match value {
        Value::Object(map) if map.contains_key("used") && map.contains_key("limit") => {
            let limit = match map["limit"].as_i64() {
                Some(-1) => "sin límite".to_owned(),
                _ => s(value, "limit"),
            };
            rows.push(vec![prefix.to_owned(), s(value, "used"), limit]);
        }
        Value::Object(map) => {
            for (k, v) in map {
                let name = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                limit_rows(&name, v, rows);
            }
        }
        _ => {}
    }
}

pub async fn usage(ctx: &Ctx, query: Option<String>) -> Result<()> {
    let api = ctx.api()?;
    let org = target(ctx, &api, query).await?;
    let data = api.get(&format!("/organizations/{}/usage", enc(&org.id))).await?;
    ctx.out.print(&data, || {
        let mut rows = Vec::new();
        limit_rows("", &data, &mut rows);
        if rows.is_empty() {
            serde_json::to_string_pretty(&data).unwrap_or_default()
        } else {
            ui::table(&["recurso", "usado", "límite"], &rows)
        }
    })
}

pub async fn members(ctx: &Ctx, query: Option<String>) -> Result<()> {
    let api = ctx.api()?;
    let org = target(ctx, &api, query).await?;
    let data = api
        .get(&format!("/organizations/{}/members", enc(&org.id)))
        .await
        .map_err(|e| crate::api::explain_scope(e, "ver los miembros"))?;
    let items = data.as_array().cloned().or_else(|| data["members"].as_array().cloned()).unwrap_or_default();
    ctx.out.print(&data, || {
        let rows: Vec<Vec<String>> = items
            .iter()
            .map(|m| {
                let name = m["name"].as_str().or_else(|| m["user"]["name"].as_str()).unwrap_or("-").to_owned();
                let email = m["email"].as_str().or_else(|| m["user"]["email"].as_str()).unwrap_or("-").to_owned();
                vec![name, email, s(m, "role"), s(m, "user_id")]
            })
            .collect();
        ui::table(&["nombre", "email", "rol", "usuario"], &rows)
    })
}
