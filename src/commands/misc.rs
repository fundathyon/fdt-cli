//! Context (`use`, `link`, `unlink`, `context`, `status`, `doctor`), the raw
//! `api` call and `update`.

use crate::api::{Api, api_error};
use crate::config::{self, Link, OrgRef};
use crate::context::{Ctx, org_ref};
use crate::failure::{Failure, Kind, fail, failure_of, report};
use crate::output::{fields, s};
use crate::{resolve, secrets, ui};
use anyhow::{Context as _, Result};
use reqwest::Method;
use serde_json::{Value, json};
use std::io::Read as _;
use std::path::PathBuf;

pub async fn use_org(ctx: &mut Ctx, query: Option<String>) -> Result<()> {
    let api = ctx.api()?;
    let orgs = resolve::orgs(&api).await?;
    let org = match query.or_else(|| ctx.org_flag.clone()) {
        Some(q) => resolve::pick(&orgs, &q, &["name"], "organización", "fdt org list")?,
        None if ctx.interactive && !orgs.is_empty() => {
            let labels: Vec<String> = orgs.iter().map(|o| s(o, "name")).collect();
            orgs[ui::select("Organización", &labels)?].clone()
        }
        None => return Err(ctx.need("<org>", "fdt use <org>")),
    };
    let org = org_ref(&org);
    ctx.config.org = Some(org.clone());
    ctx.config.save(&ctx.home)?;
    ui::success(&format!("Organización activa: {}.", org.name));
    if let Some((path, link)) = &ctx.link
        && link.org.as_deref().is_some_and(|id| id != org.id)
    {
        ui::warn(&format!("Aquí manda {} (otra organización): fdt unlink para usar la activa.", path.display()));
    }
    ctx.out.print(&json!(org), String::new)
}

pub async fn link(ctx: &Ctx, project: Option<String>, service: Option<String>) -> Result<()> {
    let api = ctx.api()?;
    let org = ctx.org(&api).await?;
    // Everything is checked against the API before .fdt.toml is written.
    let org_data = api
        .get(&format!("/organizations/{}", crate::api::enc(&org.id)))
        .await
        .map_err(|e| e.context(format!("la organización {} no existe o no tienes acceso", org.name)))?;
    let org = OrgRef { id: org.id.clone(), name: org_data["name"].as_str().map_or(org.name.clone(), str::to_owned) };
    let project = match project {
        Some(q) => Some(resolve::project(&api, &org.id, &q).await?),
        None if ctx.interactive => {
            let projects = resolve::projects(&api, &org.id).await?;
            if projects.is_empty() {
                None
            } else {
                let mut labels: Vec<String> = projects.iter().map(|p| s(p, "name")).collect();
                labels.push("(ninguno)".into());
                let i = ui::select("Proyecto", &labels)?;
                projects.get(i).cloned()
            }
        }
        None => None,
    };
    let service = match service {
        Some(q) => Some(resolve::service(&api, &org.id, &q).await?),
        None => None,
    };
    let link = Link {
        org: Some(org.id.clone()),
        org_name: Some(org.name.clone()),
        project: project.as_ref().map(resolve::id),
        project_name: project.as_ref().map(|p| s(p, "name")),
        service: service.as_ref().map(resolve::id),
        service_name: service.as_ref().map(|v| s(v, "slug")),
    };
    let path = config::link_target(&ctx.cwd);
    config::write_link(&path, &link)?;
    ui::success(&format!("{} apunta a {}{}{}.", path.display(), org.name,
        link.project_name.as_ref().map(|p| format!(" / {p}")).unwrap_or_default(),
        link.service_name.as_ref().map(|p| format!(" / {p}")).unwrap_or_default()));
    ctx.out.print(&json!({"path": path, "link": link}), String::new)
}

pub fn unlink(ctx: &Ctx) -> Result<()> {
    let Some((path, _)) = &ctx.link else {
        ui::info("No hay .fdt.toml en esta carpeta ni en las de arriba.");
        return ctx.out.print(&json!({"removed": null}), String::new);
    };
    std::fs::remove_file(path).with_context(|| format!("no se pudo borrar {}", path.display()))?;
    ui::success(&format!("{} borrado.", path.display()));
    ctx.out.print(&json!({"removed": path}), String::new)
}

/// Where the org comes from, without calling the API.
fn org_source(ctx: &Ctx) -> (Option<String>, &'static str) {
    if let Some(flag) = &ctx.org_flag {
        return (Some(flag.clone()), "--org / FDT_ORG");
    }
    if let Some(name) = ctx.link_value(|l| l.org_name.clone().or(l.org.clone())) {
        return (Some(name), ".fdt.toml");
    }
    if let Some(org) = &ctx.config.org {
        return (Some(org.name.clone()), "fdt use");
    }
    (None, "ninguna")
}

pub fn context(ctx: &Ctx) -> Result<()> {
    let token = ctx.token()?;
    let api_source = if ctx.api_url_flag.is_some() {
        "--api-url / FDT_API_URL"
    } else if ctx.config.api_url.is_some() {
        "config.toml"
    } else {
        "por defecto"
    };
    let cred = token.as_ref().and_then(|t| t.key.as_ref()).and_then(|k| ctx.config.credentials.get(k));
    let (org, org_from) = org_source(ctx);
    let data = json!({
        "version": crate::VERSION,
        "api_url": ctx.api_url()?, "api_url_source": api_source,
        "token": {
            "present": token.is_some(), "source": token.as_ref().map(|t| t.source),
            "kind": cred.and_then(|c| c.kind.clone()), "user": cred.and_then(|c| c.user.clone()),
            "organization_id": cred.and_then(|c| c.organization_id.clone()),
            "expires_at": cred.and_then(|c| c.expires_at.clone()),
        },
        "org": org, "org_source": org_from,
        "project": ctx.link_value(|l| l.project_name.clone().or(l.project.clone())),
        "service": ctx.link_value(|l| l.service_name.clone().or(l.service.clone())),
        "link": ctx.link.as_ref().map(|(p, _)| p.clone()),
        "config_dir": ctx.home, "working_directory": ctx.cwd,
        "keyring": secrets::keyring_available(),
    });
    ctx.out.print(&data, || {
        fields(&[
            ("api", format!("{} ({api_source})", s(&data, "api_url"))),
            ("token", match &token {
                Some(t) => format!("{} · {}", t.source, cred.and_then(|c| c.kind.clone()).unwrap_or_else(|| "?".into())),
                None => "ninguno (fdt login)".into(),
            }),
            ("organización", format!("{} ({org_from})", s(&data, "org"))),
            ("proyecto", s(&data, "project")),
            ("servicio", s(&data, "service")),
            ("link", s(&data, "link")),
        ])
    })
}

pub async fn status(ctx: &Ctx, project: Option<String>) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let project = ctx.project_opt(&api, &org, project.as_deref()).await?;
    let services = super::service::of_project(&api, &org.id, project.as_ref()).await?;
    let databases = match &project {
        Some(p) => api
            .get(&format!("/organizations/{}/projects/{}/databases", crate::api::enc(&org.id), crate::api::enc(&resolve::id(p))))
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default(),
        None => resolve::databases(&api, &org.id).await?,
    };
    let data = json!({"org": org, "project": project, "services": services, "databases": databases});
    ctx.out.print(&data, || {
        let header = format!(
            "{}{}",
            org.name,
            project.as_ref().map(|p| format!(" / {}", s(p, "name"))).unwrap_or_default()
        );
        let dbs: Vec<Vec<String>> = databases.iter().map(|d| vec![s(d, "name"), s(d, "engine"), ui::status(&s(d, "status"))]).collect();
        format!(
            "{}\n\n{}\n\n{}",
            ui::Colors::stdout().bold(&header),
            if services.is_empty() { "Sin servicios.".to_owned() } else { ui::table(&super::service::HEADERS, &super::service::rows(&services)) },
            if dbs.is_empty() { "Sin bases de datos.".to_owned() } else { ui::table(&["base", "motor", "estado"], &dbs) }
        )
    })
}

pub async fn doctor(ctx: &Ctx) -> Result<()> {
    let mut checks = Vec::new();
    let mut first_failure: Option<Kind> = None;
    let mut check = |name: &str, result: Result<String>| {
        match result {
            Ok(detail) => checks.push(json!({"check": name, "ok": true, "detail": detail})),
            Err(e) => {
                let (r, _) = report(&e);
                first_failure.get_or_insert(failure_of(&e).map_or(Kind::Error, |f| f.kind));
                checks.push(json!({"check": name, "ok": false, "error": r["error"]}));
            }
        }
    };
    check("version", Ok(crate::VERSION.to_owned()));
    check("api_url", ctx.api_url());
    check("keyring", Ok(if secrets::keyring_available() { "llavero del sistema".into() } else { "archivo 0600 (sin llavero)".into() }));
    let link = match &ctx.link {
        Some((p, _)) => Ok(p.display().to_string()),
        None => Ok("sin .fdt.toml (opcional)".into()),
    };
    check("link", link);
    let api = match ctx.api() {
        Ok(api) => Some(api),
        Err(e) => {
            check("token", Err(e));
            None
        }
    };
    if let Some(api) = api {
        let token = ctx.token()?.map(|t| t.source).unwrap_or("-");
        check("token", Ok(token.to_owned()));
        let who = api.get("/cli/whoami").await;
        let ok = who.is_ok();
        check("whoami", who.map(|w| s(&w["user"], "email")));
        if ok {
            check("org", ctx.org(&api).await.map(|o: OrgRef| format!("{} ({})", o.name, o.id)));
        }
    }
    let ok = first_failure.is_none();
    let data = json!({"ok": ok, "checks": checks});
    ctx.out.print(&data, || {
        let c = ui::Colors::stdout();
        checks
            .iter()
            .map(|ch| {
                let mark = if ch["ok"] == true { c.green("✔") } else { c.red("✖") };
                let detail = if ch["ok"] == true { s(ch, "detail") } else { s(&ch["error"], "message") };
                format!("{mark} {:<8} {detail}", s(ch, "check"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    match first_failure {
        None => Ok(()),
        Some(kind) => Err(Failure::new(kind, "fdt doctor encontró problemas (mira los checks).").into()),
    }
}

/// Replaces `{org}`, `{project}` and `{service}` with ids from the context.
async fn fill(ctx: &Ctx, api: &Api, path: &str) -> Result<String> {
    let mut out = path.to_owned();
    if out.contains("{org}") || out.contains("{project}") || out.contains("{service}") {
        let org = ctx.org(api).await?;
        if out.contains("{project}") {
            let p = ctx.project(api, &org, None).await?;
            out = out.replace("{project}", &resolve::id(&p));
        }
        if out.contains("{service}") {
            let v = ctx.service(api, &org, None).await?;
            out = out.replace("{service}", &resolve::id(&v));
        }
        out = out.replace("{org}", &org.id);
    }
    Ok(out)
}

pub async fn api(
    ctx: &Ctx,
    method: String,
    path: String,
    data: Option<String>,
    input: Option<PathBuf>,
    query: Vec<String>,
) -> Result<()> {
    let method = match method.to_uppercase().as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "HEAD" => Method::HEAD,
        other => return Err(fail(Kind::Usage, format!("Método no soportado: {other}"))),
    };
    let raw = match (data, input) {
        (Some(d), _) => Some(d),
        (None, Some(p)) if p.as_os_str() == "-" => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            Some(s)
        }
        (None, Some(p)) => Some(std::fs::read_to_string(&p).with_context(|| format!("no se pudo leer {}", p.display()))?),
        (None, None) => None,
    };
    let body: Option<Value> = match raw {
        Some(r) => Some(serde_json::from_str(&r).map_err(|e| fail(Kind::Usage, format!("El cuerpo no es JSON válido: {e}")))?),
        None => None,
    };
    let query: Vec<(String, String)> = query
        .iter()
        .map(|q| {
            q.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .ok_or_else(|| fail(Kind::Usage, format!("«{q}» no es k=v")))
        })
        .collect::<Result<_>>()?;
    let api = ctx.api()?;
    let path = fill(ctx, &api, &path).await?;
    let path = if path.starts_with('/') { path } else { format!("/{path}") };
    let answer = api.raw(method, &path, &query, body.as_ref()).await?;
    match &answer.body {
        Value::Null => {}
        Value::String(text) => println!("{text}"),
        other => crate::output::print_json(other)?,
    }
    if (200..300).contains(&answer.status) {
        Ok(())
    } else {
        Err(api_error(answer.status, &answer.body).into())
    }
}

pub async fn update(ctx: &Ctx, check: bool) -> Result<()> {
    use crate::update::Status;
    let (status, current, latest) = crate::update::update(crate::VERSION, check).await?;
    let label = match status {
        Status::UpToDate => "up-to-date",
        Status::Available => "available",
        Status::Updated => "updated",
    };
    match status {
        Status::UpToDate => ui::success(&format!("fdt {current} ya es la última versión.")),
        Status::Available => ui::info(&format!("Hay una versión nueva: {latest} (tienes {current}). Instálala con `fdt update`.")),
        Status::Updated => ui::success(&format!("fdt actualizado: {current} → {latest}.")),
    }
    ctx.out.print(&json!({"status": label, "current": current, "latest": latest}), String::new)
}
