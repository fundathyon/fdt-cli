//! `fdt service list|show|create|update|delete|setup|diagnose`.

use super::{service_path, services_path};
use crate::api::enc;
use crate::cli::{ServiceCreate, ServiceUpdate};
use crate::context::{Ctx, dry_run};
use crate::envfile;
use crate::failure::{Kind, fail};
use crate::output::{fields, path, s, when};
use crate::{resolve, ui};
use anyhow::{Context as _, Result};
use reqwest::Method;
use serde_json::{Value, json};

pub fn rows(services: &[Value]) -> Vec<Vec<String>> {
    services
        .iter()
        .map(|v| {
            vec![
                s(v, "slug"),
                ui::status(&s(v, "status")),
                s(v, "url"),
                ui::status(&path(v, "latest_deployment.status")),
                when(&v["latest_deployment"], "created_at"),
            ]
        })
        .collect()
}

pub const HEADERS: [&str; 5] = ["servicio", "estado", "url", "último deploy", "cuándo"];

/// Services of the org, or only those whose app is in `project`.
pub async fn of_project(api: &crate::api::Api, org: &str, project: Option<&Value>) -> Result<Vec<Value>> {
    let services = resolve::services(api, org).await?;
    let Some(project) = project else { return Ok(services) };
    let apps: Vec<String> = resolve::project_apps(api, org, &resolve::id(project)).await?.iter().map(resolve::id).collect();
    Ok(services.into_iter().filter(|s| s["app_id"].as_str().is_some_and(|a| apps.iter().any(|x| x == a))).collect())
}

pub async fn list(ctx: &Ctx, project: Option<String>) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let project = ctx.project_opt(&api, &org, project.as_deref()).await?;
    let services = Value::Array(of_project(&api, &org.id, project.as_ref()).await?);
    ctx.out.print(&services, || ui::table(&HEADERS, &rows(services.as_array().map_or(&[], |v| v))))
}

pub fn detail(v: &Value) -> String {
    fields(&[
        ("servicio", format!("{} ({})", s(v, "name"), s(v, "slug"))),
        ("id", s(v, "id")),
        ("estado", ui::status(&s(v, "status"))),
        ("url", s(v, "url")),
        ("repo", path(v, "app.repository_full_name")),
        ("rama", s(v, "branch")),
        ("puerto", s(v, "port")),
        ("memoria", format!("{} MB", s(v, "memory_mb"))),
        ("healthcheck", s(v, "health_path")),
        ("dockerfile", format!("{} (contexto {})", s(v, "dockerfile_path"), s(v, "build_context"))),
        ("workflow", format!("{} · {}", s(v, "workflow_mode"), s(v, "workflow_status"))),
        ("auto deploy", s(v, "auto_deploy")),
        ("imagen", s(v, "image_reference")),
        (
            "último deploy",
            format!(
                "{} {} {}",
                ui::status(&path(v, "latest_deployment.status")),
                path(v, "latest_deployment.source"),
                when(&v["latest_deployment"], "created_at")
            ),
        ),
    ])
}

pub async fn show(ctx: &Ctx, query: Option<String>) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let data = api.get(&service_path(&org.id, &resolve::id(&service))).await?;
    ctx.out.print(&data, || detail(&data))
}

fn pairs(items: &[String], secret: bool) -> Result<Vec<Value>> {
    items
        .iter()
        .map(|item| {
            let (k, v) = item
                .split_once('=')
                .ok_or_else(|| fail(Kind::Usage, format!("«{item}» no es KEY=VALUE")))?;
            if !envfile::valid_key(k) {
                return Err(fail(Kind::Usage, format!("nombre de variable inválido: {k}")));
            }
            Ok(json!({"key": k, "value": v, "is_secret": secret}))
        })
        .collect()
}

pub async fn create(ctx: &Ctx, a: ServiceCreate) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    if a.if_not_exists {
        let services = resolve::services(&api, &org.id).await?;
        if let Some(existing) = services.iter().find(|v| v["slug"].as_str() == Some(a.slug.trim())) {
            ui::info(&format!("El servicio «{}» ya existía.", a.slug));
            return ctx.out.print(existing, || detail(existing));
        }
    }
    let app = match (&a.repo, &a.app) {
        (Some(q), _) | (None, Some(q)) => resolve::app(&resolve::apps(&api, &org.id).await?, q)?,
        (None, None) if ctx.interactive => {
            let apps = resolve::apps(&api, &org.id).await?;
            if apps.is_empty() {
                return Err(fail(Kind::NotFound, "La organización no tiene apps: vincula GitHub en el panel."));
            }
            let labels: Vec<String> = apps.iter().map(|x| path(x, "repository.full_name")).collect();
            apps[ui::select("Repo", &labels)?].clone()
        }
        (None, None) => return Err(ctx.need("--repo <owner/nombre> (o --app)", "el servicio despliega el repo de una app")),
    };
    let project = match &a.project {
        Some(p) => Some(resolve::project(&api, &org.id, p).await?),
        None => None,
    };
    let mut env = pairs(&a.env, false)?;
    env.extend(pairs(&a.secrets, true)?);
    if !a.secrets.is_empty() {
        ui::warn("Los valores de --secret quedan en el historial del shell y en la lista de procesos: usa --secret-from-env o --secret-from-file.");
    }
    for item in &a.secrets_from_env {
        let (key, var) = item.split_once('=').unwrap_or((item.as_str(), item.as_str()));
        if !envfile::valid_key(key) {
            return Err(fail(Kind::Usage, format!("nombre de variable inválido: {key}")));
        }
        let value = std::env::var(var)
            .map_err(|_| ctx.need(&format!("la variable de entorno {var}"), &format!("--secret-from-env {item} la lee de tu entorno")))?;
        env.push(json!({"key": key, "value": value, "is_secret": true}));
    }
    for item in &a.secrets_from_file {
        let (key, file) = item
            .split_once('=')
            .ok_or_else(|| fail(Kind::Usage, format!("«{item}» no es KEY=RUTA")))?;
        if !envfile::valid_key(key) {
            return Err(fail(Kind::Usage, format!("nombre de variable inválido: {key}")));
        }
        let value = std::fs::read_to_string(file).with_context(|| format!("no se pudo leer {file}"))?;
        env.push(json!({"key": key, "value": value.trim_end_matches(['\n', '\r']), "is_secret": true}));
    }
    let mut body = json!({
        "app_id": resolve::id(&app), "slug": a.slug.trim(), "trigger": "branch",
        "branch": a.branch, "port": a.port, "memory_mb": a.memory,
    });
    for (key, value) in [
        ("name", a.name.clone()),
        ("health_path", a.health_path.clone()),
        ("dockerfile_path", a.dockerfile.clone()),
        ("build_context", a.build_context.clone()),
        ("workflow_mode", a.workflow_mode.clone()),
    ] {
        if let Some(v) = value {
            body[key] = json!(v);
        }
    }
    if a.no_auto_deploy {
        body["auto_deploy"] = json!(false);
    }
    if !env.is_empty() {
        body["env"] = json!(env);
    }
    let link_path = project
        .as_ref()
        .map(|p| format!("/organizations/{}/projects/{}/apps", enc(&org.id), enc(&resolve::id(p))));
    if a.dry_run {
        let mut shown = body.clone();
        // get_mut, not shown["env"]: indexing a missing key inserts "env": null,
        // and the dry run would show a field the real request never sends.
        if let Some(vars) = shown.get_mut("env").and_then(Value::as_array_mut) {
            for v in vars.iter_mut().filter(|v| v["is_secret"] == true) {
                v["value"] = json!("••••••");
            }
        }
        if let Some(link) = &link_path {
            ui::info(&format!("También: POST {link} {{app_id}}"));
        }
        // POST …/deploy/services keeps the slug as given: the URL is
        // https://<slug>.<domain>. The <prefix>-<suffix> form of
        // …/deploy/subdomain is only for …/deploy/configure (the panel).
        if let Ok(sub) = api.get(&format!("/organizations/{}/deploy/subdomain", enc(&org.id))).await {
            ui::info(&format!("URL: https://{}.{}", a.slug.trim(), s(&sub, "domain")));
        }
        return dry_run(ctx, "service create", "POST", &services_path(&org.id), Some(shown));
    }
    if let Some(link) = &link_path {
        api.post(link, &json!({"app_id": resolve::id(&app)}))
            .await
            .map_err(|e| crate::api::explain_scope(e, "poner apps en proyectos"))
            .context("no se pudo poner la app en el proyecto")?;
    }
    let data = api
        .post(&services_path(&org.id), &body)
        .await
        .map_err(|e| crate::api::explain_scope(e, "crear servicios"))?;
    ui::success(&format!("Servicio «{}» creado: {}", s(&data, "slug"), s(&data, "url")));
    ui::info(&format!(
        "Siguiente: fdt service install-workflow {} --mode pull_request, y fdt deploy {}",
        s(&data, "slug"),
        s(&data, "slug")
    ));
    ctx.out.print(&data, || detail(&data))
}

pub async fn update(ctx: &Ctx, a: ServiceUpdate) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, a.service.as_deref()).await?;
    let mut body = serde_json::Map::new();
    let mut put = |k: &str, v: Value| {
        body.insert(k.to_owned(), v);
    };
    if let Some(v) = a.name {
        put("name", json!(v));
    }
    if let Some(v) = a.branch {
        put("branch", json!(v));
    }
    if let Some(v) = a.port {
        put("port", json!(v));
    }
    if let Some(v) = a.memory {
        put("memory_mb", json!(v));
    }
    if let Some(v) = a.health_path {
        put("health_path", if v.trim().is_empty() { Value::Null } else { json!(v) });
    }
    if let Some(v) = a.dockerfile {
        put("dockerfile_path", json!(v));
    }
    if let Some(v) = a.build_context {
        put("build_context", json!(v));
    }
    if let Some(v) = a.auto_deploy {
        put("auto_deploy", json!(v));
    }
    if let Some(v) = a.workflow_mode {
        put("workflow_mode", json!(v));
    }
    if body.is_empty() {
        return Err(fail(Kind::Usage, "No hay nada que cambiar: pasa al menos un flag (fdt service update --help)."));
    }
    let path = service_path(&org.id, &resolve::id(&service));
    let body = Value::Object(body);
    if a.dry_run {
        return dry_run(ctx, "service update", "PATCH", &path, Some(body));
    }
    let data = api.patch(&path, &body).await?;
    ui::success(&format!("Servicio «{}» actualizado.", s(&data, "slug")));
    ctx.out.print(&data, || detail(&data))
}

pub async fn delete(ctx: &Ctx, query: String, confirm: Option<String>, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = resolve::service(&api, &org.id, &query).await?;
    let slug = s(&service, "slug");
    let path = service_path(&org.id, &resolve::id(&service));
    if dry {
        return dry_run(ctx, "service delete", "DELETE", &path, None);
    }
    ctx.confirm(&slug, confirm.as_deref(), "borrar el servicio")?;
    let data = api.delete(&path).await?;
    ui::success(&format!("Servicio «{slug}» en borrado (deleting): desaparece cuando el deployer confirma."));
    ctx.out.print(&data, String::new)
}

/// Read-only: what the repo needs. Writing the workflow locally is the only side effect.
pub async fn setup(ctx: &Ctx, query: Option<String>, write: Option<std::path::PathBuf>) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let base = service_path(&org.id, &resolve::id(&service));
    let setup = api.get(&format!("{base}/setup")).await?;
    let mut out = json!({"setup": setup});
    if let Some(file) = write {
        let yaml = setup["workflow_yaml"].as_str().ok_or_else(|| fail(Kind::Api, "La API no devolvió workflow_yaml."))?;
        let target = if file.is_absolute() { file } else { ctx.cwd.join(file) };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, yaml).with_context(|| format!("no se pudo escribir {}", target.display()))?;
        ui::success(&format!("Workflow escrito en {}. Haz commit y push (y guarda el secreto {} en el repo).", target.display(), s(&setup, "secret_name")));
        out["written"] = json!(target);
    }
    ctx.out.print(&out, || {
        fields(&[
            ("servicio", s(&service, "slug")),
            ("workflow", s(&setup, "workflow_path")),
            ("imagen", s(&setup, "image_repository")),
            ("registry", format!("{} (usuario {})", s(&setup, "registry_host"), s(&setup, "registry_username"))),
            ("secreto del repo", s(&setup, "secret_name")),
            ("token vivo", if setup["registry_token"].is_null() { "ninguno".into() } else { path(&setup, "registry_token.id") }),
            ("Dockerfile sugerido", serde_json::to_string(&setup["recommended_dockerfile"]).unwrap_or_default()),
        ])
    })
}

pub async fn install_workflow(ctx: &Ctx, query: Option<String>, mode: String, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let path_ = format!("{}/workflow", service_path(&org.id, &resolve::id(&service)));
    let body = json!({"workflow_mode": mode});
    if mode == "commit" {
        ui::warn(&format!(
            "Con --mode commit Core hace commit del workflow en la rama {}: ese push dispara un build (y un deploy si auto_deploy está activo), y tu copia local queda atrás: haz git pull antes de seguir.",
            s(&service, "branch")
        ));
    } else {
        ui::info("Con --mode pull_request Core abre un pull request; el build empieza cuando lo fusiones.");
    }
    if dry {
        return dry_run(ctx, "service install-workflow", "POST", &path_, Some(body));
    }
    let result = api.post(&path_, &body).await?;
    let error = &result["workflow"]["error"];
    if error.is_null() {
        ui::success(&format!("Workflow instalado ({}).", path(&result, "workflow.status")));
        if let Some(url) = result["workflow"]["pull_request_url"].as_str() {
            ui::info(&format!("Pull request: {url}"));
        }
    } else {
        return Err(crate::failure::Failure::new(
            Kind::Api,
            format!("GitHub no aceptó el workflow: {} ({})", s(error, "message"), s(error, "code")),
        )
        .details(json!({"workflow": result["workflow"]}))
        .hint(format!("fdt service diagnose {}", s(&service, "slug")))
        .into());
    }
    ctx.out.print(&result, String::new)
}

pub async fn rotate_registry_token(
    ctx: &Ctx,
    query: String,
    confirm: Option<String>,
    reveal: bool,
    output: Option<std::path::PathBuf>,
    dry: bool,
) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = resolve::service(&api, &org.id, &query).await?;
    let slug = s(&service, "slug");
    let path_ = format!("{}/registry-token", service_path(&org.id, &resolve::id(&service)));
    ui::warn("Rotar crea un token fdr_ nuevo y REVOCA el anterior: el workflow del repo no podrá publicar imágenes hasta que guardes el nuevo en su secreto.");
    if dry {
        return dry_run(ctx, "service rotate-registry-token", "POST", &path_, None);
    }
    if !reveal && output.is_none() {
        return Err(crate::failure::Failure::new(
            Kind::MissingInput,
            "El token nuevo se muestra una sola vez: indica --output <archivo> (0600) o --reveal.",
        )
        .hint(format!("fdt service rotate-registry-token {slug} --confirm {slug} --output .fdr-token"))
        .into());
    }
    ctx.confirm(&slug, confirm.as_deref(), "revocar y rotar el token del registry del servicio")?;
    let mut token = api.call(Method::POST, &path_, None).await?;
    if let Some(file) = &output {
        crate::secrets::write_private(file, &format!("{}\n", s(&token, "secret")))?;
        ui::success(&format!("Token nuevo en {} (0600).", file.display()));
        token["secret"] = json!(crate::secrets::MASK);
        token["secret_file"] = json!(file);
    }
    let setup = api.get(&format!("{}/setup", service_path(&org.id, &resolve::id(&service)))).await.unwrap_or_default();
    ui::warn(&format!(
        "Guarda el token nuevo en el secreto {} del repo (o reinstala el workflow: fdt service install-workflow {slug}).",
        s(&setup, "secret_name")
    ));
    ctx.out.print(&token, || {
        fields(&[
            ("token", s(&token, "secret")),
            ("usuario", s(&token, "username")),
            ("registry", s(&token, "registry_host")),
            ("repositorio", s(&token, "repository")),
        ])
    })
}

/// `restart`, `pause` (destructive: confirm) and `resume` (may wait).
pub async fn lifecycle(
    ctx: &Ctx,
    query: Option<String>,
    action: &str,
    confirm: Option<String>,
    wait: Option<crate::cli::Wait>,
    dry: bool,
) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let base = service_path(&org.id, &resolve::id(&service));
    let path_ = format!("{base}/{action}");
    if dry {
        if action == "pause" {
            ui::warn("Pausar deja el servicio en 0 contenedores: deja de responder hasta fdt service resume.");
        }
        return dry_run(ctx, &format!("service {action}"), "POST", &path_, None);
    }
    if action == "pause" {
        ctx.confirm(&slug, confirm.as_deref(), "pausar (0 contenedores) el servicio")?;
    }
    let data = api.call(Method::POST, &path_, None).await?;
    match action {
        "restart" => ui::success(&format!("{slug} reiniciado con la misma imagen.")),
        "pause" => ui::success(&format!("{slug} pausado (0 contenedores). Reanuda con fdt service resume {slug}.")),
        _ => ui::info(&format!("{slug}: deployment {} en cola para reanudar.", s(&data, "id"))),
    }
    if action == "resume"
        && let Some(w) = wait.filter(|w| w.wait)
    {
        let done = super::deploy::wait_for(&api, &base, &resolve::id(&data), &slug, std::time::Instant::now() + w.timeout).await?;
        ui::success(&format!("{slug} está live."));
        return ctx.out.print(&done, String::new);
    }
    ctx.out.print(&data, String::new)
}

pub async fn diagnose(ctx: &Ctx, query: Option<String>, refresh: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let q = if refresh { vec![("refresh".to_owned(), "true".to_owned())] } else { vec![] };
    let data = api.get_query(&format!("{}/diagnostics", service_path(&org.id, &resolve::id(&service))), &q).await?;
    ctx.out.print(&data, || {
        let rows: Vec<Vec<String>> = data["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| vec![s(c, "id"), ui::status(&s(c, "status")), s(c, "code")])
            .collect();
        format!(
            "{}\n\n{}\n{}",
            ui::table(&["check", "estado", "código"], &rows),
            s(&data, "summary"),
            data["next_step"].as_str().map(|n| format!("Siguiente paso: {n}")).unwrap_or_default()
        )
    })
}
