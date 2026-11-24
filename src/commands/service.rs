//! `fdt service list|show|create|update|delete|setup|diagnose`.

use super::service_path;
use crate::api::enc;
use crate::cli::{ServiceCreate, ServiceUpdate};
use crate::context::{Ctx, dry_run, dry_run_with};
use crate::envfile;
use crate::failure::{Failure, Kind, fail};
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

/// Subdomains that belong to Foundathyon: Core's `ReservedSlugs`
/// (internal/context/v1/deploy/domain/entities). 400 `deploy.slug_reserved`.
pub const RESERVED_SLUGS: [&str; 25] = [
    "www", "api", "admin", "app", "db", "registry", "mail", "status", "docs", "foundathyon", "accounts", "auth", "core",
    "gateway", "dashboard", "cdn", "static", "assets", "help", "support", "blog", "smtp", "ftp", "ns1", "ns2",
];

/// Core's `SubdomainPrefixRe`: [a-z0-9-], at least one, no hyphen at either end.
fn valid_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix.starts_with('-')
        && !prefix.ends_with('-')
        && prefix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Core's `SlugProblem`: a DNS label of 3 to 63 (`SlugRe`), and not reserved.
pub fn slug_problem(slug: &str) -> Option<&'static str> {
    let label = slug.len() >= 3
        && slug.len() <= 63
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !label {
        Some("invalid")
    } else if RESERVED_SLUGS.contains(&slug) {
        Some("reserved")
    } else {
        None
    }
}

/// The subdomain checks Core makes, before sending anything. `suffix` is the
/// org's (`GET …/deploy/subdomain`), when fdt could read it.
fn check_subdomain(prefix: &str, suffix: Option<&str>) -> Result<Option<String>> {
    let reserved = || {
        let mut f = Failure::new(
            Kind::Usage,
            format!("El subdominio «{prefix}» es de Foundathyon (reservado): elige otro."),
        )
        .hint("www, api, admin, app, db, registry, docs, auth… están reservados: p. ej. tienda-web".to_owned());
        f.scope = Some("deploy.slug_reserved".into());
        anyhow::Error::from(f)
    };
    if RESERVED_SLUGS.contains(&prefix) && suffix.is_none() {
        return Err(reserved());
    }
    if !valid_prefix(prefix) {
        let mut f = Failure::new(
            Kind::Usage,
            format!("Subdominio inválido «{prefix}»: solo a-z, 0-9 y -, sin guion al principio ni al final."),
        );
        f.scope = Some("deploy.subdomain_invalid".into());
        return Err(f.into());
    }
    let Some(suffix) = suffix else { return Ok(None) };
    let slug = format!("{prefix}-{suffix}");
    match slug_problem(&slug) {
        Some("reserved") => Err(reserved()),
        Some(_) => {
            let mut f = Failure::new(
                Kind::Usage,
                format!("El subdominio «{slug}» no es válido: con el sufijo de la organización tiene que tener de 3 a 63 caracteres."),
            );
            f.scope = Some("deploy.subdomain_invalid".into());
            Err(f.into())
        }
        None => Ok(Some(slug)),
    }
}

/// The memory sizes the org's plan allows (`meta.limits.service_memory_mb_options`), if the API says.
fn memory_options(meta: &Value) -> Vec<u64> {
    meta["limits"]["service_memory_mb_options"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

fn check_memory(memory: u32, options: &[u64]) -> Result<()> {
    if options.is_empty() || options.contains(&u64::from(memory)) {
        return Ok(());
    }
    let list = options.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
    let mut f = Failure::new(
        Kind::Quota,
        format!("El plan de la organización no permite {memory} MB por servicio: elige {list} (--memory)."),
    )
    .hint(format!("--memory {} · fdt org usage (y sube de plan en el panel)", options[0]))
    .details(json!({"memory_mb": memory, "service_memory_mb_options": options}));
    f.scope = Some("quota.deploy.service_memory".into());
    Err(f.into())
}

/// The org's subdomain suffix and apps domain (`GET …/deploy/subdomain`); any member may read it.
async fn subdomain_info(api: &crate::api::Api, org: &str) -> Option<(String, String)> {
    let sub = api.get(&format!("/organizations/{}/deploy/subdomain", enc(org))).await.ok()?;
    Some((sub["suffix"].as_str()?.to_owned(), sub["domain"].as_str()?.to_owned()))
}

/// Core's `SlugBase`: the repo name as [a-z0-9-], at most 23 characters, or `app`.
fn slug_base(repo: &str) -> String {
    let mut base = String::new();
    for c in repo.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            base.push(c);
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let mut base = base.trim_matches('-').to_owned();
    base.truncate(23);
    let base = base.trim_end_matches('-').to_owned();
    if base.is_empty() { "app".into() } else { base }
}

fn configure_path(org: &str, app: &str) -> String {
    format!("/organizations/{}/apps/{}/deploy/configure", enc(org), enc(app))
}

/// The service of the app, if it has one: Core keeps one per app
/// (`configure` would update it), whatever suffix its subdomain got.
fn service_of_app<'a>(services: &'a [Value], app: &Value) -> Option<&'a Value> {
    let id = resolve::id(app);
    services
        .iter()
        .filter(|v| v["status"] != "deleting")
        .find(|v| v["app_id"].as_str() == Some(id.as_str()))
        .or_else(|| {
            let linked = app["deploy_service"]["id"].as_str()?;
            services.iter().find(|v| v["id"].as_str() == Some(linked))
        })
}

/// Creates the app's service the way the panel does: `POST …/apps/{app}/deploy/configure`.
pub async fn create(ctx: &Ctx, a: ServiceCreate) -> Result<()> {
    let prefix = a.subdomain.as_deref().map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty());
    // Before any call: a malformed subdomain never reaches Core.
    if let Some(p) = &prefix {
        check_subdomain(p, None).or_else(|e| match crate::failure::failure_of(&e) {
            // `api` alone is reserved, `api-<suffix>` is not: decide once the suffix is known.
            Some(f) if f.scope.as_deref() == Some("deploy.slug_reserved") => Ok(None),
            _ => Err(e),
        })?;
    }
    if a.port == 0 {
        return Err(fail(Kind::Usage, "--port tiene que estar entre 1 y 65535."));
    }
    let (api, org) = super::org_api(ctx).await?;
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
    let repo = app["repository"]["full_name"].as_str().map_or_else(|| s(&app, "name"), str::to_owned);
    let (services, meta) = resolve::services_with_meta(&api, &org.id).await?;
    if let Some(existing) = service_of_app(&services, &app) {
        if a.if_not_exists {
            ui::info(&format!("La app {repo} ya tenía el servicio «{}» ({}).", s(existing, "slug"), s(existing, "url")));
            let mut out = existing.clone();
            out["created"] = json!(false);
            return ctx.out.print(&out, || detail(existing));
        }
        let mut f = Failure::new(
            Kind::Error,
            format!(
                "La app {repo} ya tiene el servicio «{}» ({}): una app tiene un solo servicio.",
                s(existing, "slug"),
                s(existing, "url")
            ),
        )
        .hint(format!("--if-not-exists para reutilizarlo, o fdt service update {} para cambiarlo", s(existing, "slug")))
        .details(json!({"service": existing}));
        f.scope = Some("fdt.service_exists".into());
        return Err(f.into());
    }
    let options = memory_options(&meta);
    check_memory(a.memory, &options)?;
    let sub = subdomain_info(&api, &org.id).await;
    // Without the suffix (the read failed) Core makes the last check.
    let slug = match (&prefix, &sub) {
        (Some(p), Some((suffix, _))) => check_subdomain(p, Some(suffix))?,
        _ => None,
    };
    if let Some(taken) = slug.as_ref().and_then(|sl| services.iter().find(|v| v["slug"].as_str() == Some(sl.as_str()))) {
        let mut f = Failure::new(
            Kind::Error,
            format!("El subdominio «{}» ya es del servicio «{}» de otra app.", s(taken, "slug"), s(taken, "name")),
        )
        .hint("elige otro subdominio".to_owned());
        f.scope = Some("deploy.slug_taken".into());
        return Err(f.into());
    }

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
        "port": a.port, "memory_mb": a.memory, "auto_deploy": !a.no_auto_deploy, "workflow_mode": a.workflow_mode,
    });
    for (key, value) in [
        ("subdomain", prefix.clone()),
        ("name", a.name.clone()),
        ("branch", a.branch.clone()),
        ("health_path", a.health_path.clone()),
        ("dockerfile_path", a.dockerfile.clone()),
        ("build_context", a.build_context.clone()),
    ] {
        if let Some(v) = value {
            body[key] = json!(v);
        }
    }
    if !env.is_empty() {
        body["env"] = json!(env);
    }
    let link_path = project
        .as_ref()
        .map(|p| format!("/organizations/{}/projects/{}/apps", enc(&org.id), enc(&resolve::id(p))));
    let path_ = configure_path(&org.id, &resolve::id(&app));
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
        // configure: with a subdomain the slug is <prefix>-<org suffix>,
        // known now; without one Core draws <repo>-<8 random>, known only
        // after creating.
        let (url, certain) = match (&slug, &sub) {
            (Some(sl), Some((_, domain))) => (Some(format!("https://{sl}.{domain}")), true),
            (None, Some((_, domain))) => (Some(format!("https://{}-xxxxxxxx.{domain}", slug_base(repo.rsplit('/').next().unwrap_or(&repo)))), false),
            _ => (None, false),
        };
        match (&url, certain) {
            (Some(u), true) => ui::info(&format!("URL: {u}")),
            (Some(u), false) => ui::info(&format!(
                "URL: {u} (Core elige las 8 letras al crear; para saberla antes pasa el subdominio: fdt service create <subdominio> …)"
            )),
            _ => ui::info("URL: no se pudo leer el sufijo de la organización (GET …/deploy/subdomain); Core la dirá al crear."),
        }
        if options.is_empty() {
            ui::info(&format!("Memoria: {} MB (la API no dijo qué tamaños deja el plan).", a.memory));
        } else {
            ui::info(&format!(
                "Memoria: {} MB (el plan deja {} MB).",
                a.memory,
                options.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
            ));
        }
        match a.workflow_mode.as_str() {
            "commit" => ui::warn("Con --workflow-mode commit Core hace commit del workflow en la rama: ese push lanza el primer build (con auto_deploy) y tu copia local queda atrás."),
            "pull_request" => ui::info("Con --workflow-mode pull_request Core abre un pull request con el workflow; el primer build llega al fusionarlo."),
            _ => ui::info("Con --workflow-mode manual Core no toca GitHub: después, fdt service install-workflow."),
        }
        return dry_run_with(
            ctx,
            "service create",
            "POST",
            &path_,
            Some(shown),
            json!({"url": url, "url_certain": certain, "service_memory_mb_options": options, "repo": repo}),
        );
    }
    if let Some(link) = &link_path {
        api.post(link, &json!({"app_id": resolve::id(&app)}))
            .await
            .map_err(|e| crate::api::explain_scope(e, "poner apps en proyectos"))
            .context("no se pudo poner la app en el proyecto")?;
    }
    let data = api
        .post(&path_, &body)
        .await
        .map_err(|e| crate::api::explain_scope(e, "crear servicios"))?;
    let service = &data["service"];
    let workflow = &data["workflow"];
    let slug = s(service, "slug");
    ui::success(&format!("Servicio «{slug}» creado: {}", s(service, "url")));
    match workflow["status"].as_str().unwrap_or_default() {
        "committed" => ui::info(&format!("Workflow en la rama {} (commit {}): el build arranca solo; síguelo con fdt deploys {slug}.", s(service, "branch"), path(workflow, "commit_sha").chars().take(7).collect::<String>())),
        "pull_request_open" => ui::info(&format!("Pull request con el workflow: {}. Al fusionarlo llega el primer build (fdt deploys {slug}).", s(workflow, "pull_request_url"))),
        "failed" => ui::warn(&format!(
            "El servicio quedó creado, pero GitHub no aceptó el workflow: {} ({}). Reintenta con fdt service install-workflow {slug} --mode pull_request · fdt service diagnose {slug}",
            path(workflow, "error.message"),
            path(workflow, "error.code")
        )),
        _ => ui::info(&format!("Siguiente: fdt service install-workflow {slug} --mode pull_request, y fdt deploy {slug}")),
    }
    let mut out = service.clone();
    out["workflow"] = workflow.clone();
    out["created"] = json!(true);
    ctx.out.print(&out, || detail(service))
}

pub async fn update(ctx: &Ctx, a: ServiceUpdate) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, a.service.get().as_deref()).await?;
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
        let (_, meta) = resolve::services_with_meta(&api, &org.id).await?;
        check_memory(v, &memory_options(&meta))?;
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

pub async fn delete(ctx: &Ctx, query: Option<String>, confirm: Option<String>, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    // The slug in --confirm is what protects it, also with the linked service.
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let path = service_path(&org.id, &resolve::id(&service));
    // Also in a dry run: it would pass where the real one is refused otherwise.
    ctx.confirm(&slug, confirm.as_deref(), "borrar el servicio")?;
    if dry {
        return dry_run(ctx, "service delete", "DELETE", &path, None);
    }
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

pub async fn install_workflow(ctx: &Ctx, query: Option<String>, mode: Option<String>, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let path_ = format!("{}/workflow", service_path(&org.id, &resolve::id(&service)));
    // Without --mode, the mode the service already has (commit or
    // pull_request); manual has no install, so pull_request.
    let configured = service["workflow_mode"].as_str().filter(|m| matches!(*m, "commit" | "pull_request"));
    let mode = mode.or_else(|| configured.map(str::to_owned)).unwrap_or_else(|| "pull_request".to_owned());
    if let Some(current) = configured
        && current != mode
    {
        ui::warn(&format!(
            "El servicio {} tiene workflow_mode {current}; esto lo instala con {mode} y deja el servicio en {mode}.",
            s(&service, "slug")
        ));
    }
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
        return Err(Failure::new(
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
    query: Option<String>,
    confirm: Option<String>,
    reveal: bool,
    output: Option<std::path::PathBuf>,
    dry: bool,
) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let path_ = format!("{}/registry-token", service_path(&org.id, &resolve::id(&service)));
    ui::warn("Rotar crea un token fdr_ nuevo y REVOCA el anterior: el workflow del repo no podrá publicar imágenes hasta que guardes el nuevo en su secreto.");
    if dry {
        ctx.confirm(&slug, confirm.as_deref(), "revocar y rotar el token del registry del servicio")?;
        return dry_run(ctx, "service rotate-registry-token", "POST", &path_, None);
    }
    if !reveal && output.is_none() {
        return Err(Failure::new(
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
    if action == "pause" {
        if dry {
            ui::warn("Pausar deja el servicio en 0 contenedores: deja de responder hasta fdt service resume.");
        }
        // Also in a dry run: it would pass where the real one is refused otherwise.
        ctx.confirm(&slug, confirm.as_deref(), "pausar (0 contenedores) el servicio")?;
    }
    if dry {
        return dry_run(ctx, &format!("service {action}"), "POST", &path_, None);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_and_invalid_subdomains_are_refused_locally() {
        assert_eq!(slug_problem("api"), Some("reserved"));
        assert_eq!(slug_problem("ab"), Some("invalid"));
        assert_eq!(slug_problem("tienda-web"), None);
        // `api` alone is reserved; with the org suffix it is a different subdomain.
        assert!(check_subdomain("api", None).is_err());
        assert_eq!(check_subdomain("api", Some("acme")).unwrap().as_deref(), Some("api-acme"));
        assert!(check_subdomain("-web", Some("acme")).is_err());
        assert!(check_subdomain("Web_1", None).is_err());
        let long = "a".repeat(60);
        assert!(check_subdomain(&long, Some("acme")).is_err(), "more than 63 with the suffix");
    }

    #[test]
    fn memory_is_checked_against_the_plan() {
        assert!(check_memory(256, &[256]).is_ok());
        assert!(check_memory(512, &[]).is_ok(), "no options from the API: Core decides");
        let err = check_memory(512, &[256]).unwrap_err();
        let f = crate::failure::failure_of(&err).unwrap();
        assert_eq!(f.kind, Kind::Quota);
        assert_eq!(f.scope.as_deref(), Some("quota.deploy.service_memory"));
    }

    #[test]
    fn slug_base_follows_core() {
        assert_eq!(slug_base("Deploy_Test.API"), "deploy-test-api");
        assert_eq!(slug_base("a-very-long-repository-name-indeed"), "a-very-long-repository");
        assert_eq!(slug_base("__"), "app");
    }
}
