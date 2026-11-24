//! `fdt project list|show|create|rename|delete`.

use crate::api::enc;
use crate::context::{Ctx, dry_run};
use crate::failure::{Failure, Kind};
use crate::output::{fields, s, when};
use crate::{resolve, ui};
use anyhow::Result;
use serde_json::{Value, json};

pub async fn list(ctx: &Ctx) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let projects = Value::Array(resolve::projects(&api, &org.id).await?);
    ctx.out.print(&projects, || {
        let rows: Vec<Vec<String>> = projects
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let services = p["services"].as_array().map_or(0, Vec::len);
                vec![s(p, "name"), services.to_string(), when(p, "created_at"), s(p, "id")]
            })
            .collect();
        ui::table(&["proyecto", "servicios", "creado", "id"], &rows)
    })
}

pub async fn show(ctx: &Ctx, query: Option<String>) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let selected = ctx.project(&api, &org, query.as_deref()).await?;
    let mut project = resolve::pick(&resolve::projects(&api, &org.id).await?, &resolve::id(&selected), &["name"], "proyecto", "fdt project list")?;
    let pid = resolve::id(&project);
    project["apps"] = Value::Array(resolve::project_apps(&api, &org.id, &pid).await.unwrap_or_default());
    project["databases"] = api
        .get(&format!("/organizations/{}/projects/{}/databases", enc(&org.id), enc(&pid)))
        .await
        .unwrap_or(Value::Array(vec![]));
    ctx.out.print(&project, || {
        let names = |key: &str, field: &str| {
            project[key].as_array().map(|a| a.iter().map(|x| s(x, field)).collect::<Vec<_>>().join(", ")).filter(|t| !t.is_empty()).unwrap_or_else(|| "-".into())
        };
        fields(&[
            ("proyecto", s(&project, "name")),
            ("id", pid.clone()),
            ("descripción", s(&project, "description")),
            ("servicios", services(&project)),
            ("apps", names("apps", "name")),
            ("bases", names("databases", "name")),
            ("creado", when(&project, "created_at")),
        ])
    })
}

/// The project's services as Core lists them: `{type, …}` entries, with a
/// name or alias when there is one.
fn services(project: &Value) -> String {
    let items: Vec<String> = project["services"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            ["alias", "name", "slug", "type"]
                .iter()
                .find_map(|k| x[*k].as_str().filter(|v| !v.is_empty()))
                .map_or_else(|| x.as_str().unwrap_or("?").to_owned(), str::to_owned)
        })
        .collect();
    if items.is_empty() { "-".into() } else { items.join(", ") }
}

pub async fn create(ctx: &Ctx, name: String, description: Option<String>, if_not_exists: bool, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    if if_not_exists {
        let existing = resolve::projects(&api, &org.id).await?;
        let same: Vec<&Value> = existing
            .iter()
            .filter(|p| p["name"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(name.trim())))
            .collect();
        match same.as_slice() {
            [] => {}
            [p] => {
                ui::info(&format!("El proyecto «{}» ya existía.", s(p, "name")));
                return ctx.out.print(p, || format!("{} ({})", s(p, "name"), s(p, "id")));
            }
            several => {
                let candidates: Vec<String> = several.iter().map(|p| format!("{} ({})", s(p, "name"), s(p, "id"))).collect();
                return Err(Failure::new(
                    Kind::Ambiguous,
                    format!("Hay {} proyectos llamados «{}»: --if-not-exists no sabe cuál devolver.", several.len(), name.trim()),
                )
                .details(json!({"candidates": candidates}))
                .hint("usa el id del que quieras (fdt project show <id>), o renombra uno: fdt project rename <id> <nombre>")
                .into());
            }
        }
    }
    let mut body = json!({"name": name.trim()});
    if let Some(d) = description {
        body["description"] = json!(d);
    }
    let path = format!("/organizations/{}/projects", enc(&org.id));
    if dry {
        return dry_run(ctx, "project create", "POST", &path, Some(body));
    }
    let data = api.post(&path, &body).await.map_err(|e| crate::api::explain_scope(e, "crear proyectos"))?;
    ui::success(&format!("Proyecto «{}» creado en {}.", s(&data, "name"), org.name));
    ctx.out.print(&data, || s(&data, "id"))
}

pub async fn rename(ctx: &Ctx, query: String, new_name: Option<String>, description: Option<String>, dry: bool) -> Result<()> {
    if new_name.is_none() && description.is_none() {
        return Err(crate::failure::fail(
            crate::failure::Kind::Usage,
            "Indica el nombre nuevo y/o --description.",
        ));
    }
    let (api, org) = super::org_api(ctx).await?;
    let project = resolve::project(&api, &org.id, &query).await?;
    let path = format!("/organizations/{}/projects/{}", enc(&org.id), enc(&resolve::id(&project)));
    let mut body = json!({});
    if let Some(n) = &new_name {
        body["name"] = json!(n.trim());
    }
    if let Some(d) = &description {
        body["description"] = json!(d);
    }
    if dry {
        return dry_run(ctx, "project rename", "PATCH", &path, Some(body));
    }
    let data = api.patch(&path, &body).await?;
    ui::success(&format!("Proyecto «{}» actualizado.", s(&data, "name")));
    ctx.out.print(&data, String::new)
}

pub async fn delete(ctx: &Ctx, query: String, confirm: Option<String>, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let project = resolve::project(&api, &org.id, &query).await?;
    let name = s(&project, "name");
    let path = format!("/organizations/{}/projects/{}", enc(&org.id), enc(&resolve::id(&project)));
    // Also in a dry run: it would pass where the real one is refused otherwise.
    ctx.confirm(&name, confirm.as_deref(), "borrar el proyecto")?;
    if dry {
        return dry_run(ctx, "project delete", "DELETE", &path, None);
    }
    api.delete(&path).await?;
    ui::success(&format!("Proyecto «{name}» borrado."));
    ctx.out.print(&json!({"ok": true, "deleted": resolve::id(&project), "name": name}), String::new)
}

#[cfg(test)]
mod tests {
    #[test]
    fn show_names_the_services_by_the_field_core_sends() {
        let p = serde_json::json!({"services": [{"type": "deploy"}, {"type": "accounts", "alias": "Login"}]});
        assert_eq!(super::services(&p), "deploy, Login");
        assert_eq!(super::services(&serde_json::json!({"services": []})), "-");
    }
}
