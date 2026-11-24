//! `fdt deploy`, `redeploy`, `deploys` and `logs`. Waiting is polling with
//! short requests: the gateway cuts each one at 30 s and does not stream.

use super::service_path;
use crate::api::{Api, enc};
use crate::cli::Wait;
use crate::context::{Ctx, dry_run, dry_run_with};
use crate::failure::{Failure, Kind, fail};
use crate::output::{fields, s, when};
use crate::{resolve, ui};
use anyhow::Result;
use reqwest::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn poll_every() -> Duration {
    super::poll_unit() * 3
}

async fn deployments(api: &Api, base: &str) -> Result<Vec<Value>> {
    Ok(api.get(&format!("{base}/deployments")).await?.as_array().cloned().unwrap_or_default())
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// The commands that show why a deployment failed: its detail (with the
/// GitHub Actions build and the command for each job's log), the container
/// logs from when it started (empty if it never started) and the diagnosis.
pub fn failure_commands(slug: &str, d: &Value) -> Value {
    let id = s(d, "id");
    let since = [s(d, "started_at"), s(d, "created_at")].into_iter().find(|t| t != "-");
    let logs = match since {
        Some(t) => format!("fdt logs {slug} --since {t}"),
        None => format!("fdt logs {slug} --since 30m"),
    };
    json!({
        "detail": format!("fdt deploys {slug} {}", short(&id)),
        "logs": logs,
        "diagnose": format!("fdt service diagnose {slug}"),
    })
}

/// Polls a deployment until it ends. live/superseded → Ok, failed/cancelled → exit 7.
pub async fn wait_for(api: &Api, base: &str, id: &str, slug: &str, deadline: Instant) -> Result<Value> {
    let mut last = String::new();
    loop {
        let detail = api.get(&format!("{base}/deployments/{}", enc(id))).await?;
        let d = &detail["deployment"];
        let status = s(d, "status");
        let phase = s(d, "phase");
        let now = format!("{status}/{phase}");
        if now != last {
            ui::info(&format!("deploy {}: {status}{}", short(id), if phase == "-" { String::new() } else { format!(" ({phase})") }));
            last = now;
        }
        match status.as_str() {
            "live" => return Ok(d.clone()),
            "superseded" => {
                ui::warn("Otro deploy más nuevo reemplazó a este antes de terminar.");
                return Ok(d.clone());
            }
            "failed" | "cancelled" => {
                let commands = failure_commands(slug, d);
                let error = match (d["error_code"].as_str(), d["error_message"].as_str()) {
                    (Some(code), Some(m)) if !m.is_empty() => format!(": {code}: {m}"),
                    (Some(code), _) => format!(": {code}"),
                    (None, Some(m)) if !m.is_empty() => format!(": {m}"),
                    _ => String::new(),
                };
                let phase = if phase == "-" { String::new() } else { format!(" en la fase {phase}") };
                return Err(Failure::new(
                    Kind::DeployFailed,
                    format!(
                        "El deploy {} terminó en {status}{phase}{error}. Para ver por qué: {} (detalle y build, con el log de cada job), {} (logs del contenedor, si llegó a arrancar).",
                        short(id),
                        s(&commands, "detail"),
                        s(&commands, "logs"),
                    ),
                )
                .hint(format!("{} · {} · {}", s(&commands, "detail"), s(&commands, "logs"), s(&commands, "diagnose")))
                .details(json!({"deployment": d, "commands": commands}))
                .into());
            }
            _ => {}
        }
        if Instant::now() > deadline {
            return Err(Failure::new(Kind::Timeout, format!("El deploy sigue en {status} al acabar --timeout (salida 8; no falló, aún no termina)."))
                .hint(format!("fdt deploys {slug} {}", short(id)))
                .details(json!({"deployment": d}))
                .into());
        }
        tokio::time::sleep(poll_every()).await;
    }
}

/// The `workflow_run` and `image_pushed` checks of `…/diagnostics`, or None
/// if they cannot be read (a failed read never stops a deploy from waiting).
async fn build_checks(api: &Api, base: &str) -> Option<(Value, Value)> {
    let data = api.get(&format!("{base}/diagnostics")).await.ok()?;
    let checks = data["checks"].as_array()?;
    let find = |id: &str| checks.iter().find(|c| c["id"] == id).cloned();
    Some((find("workflow_run")?, find("image_pushed").unwrap_or(Value::Null)))
}

/// What the image check says about a build that finished without a new
/// deployment: Core creates none when the registry does not notify a push,
/// and it does not when the image (same tag, same digest) was already there.
#[derive(Debug, PartialEq)]
pub enum NoDeployment {
    /// The build pushed the image that already runs: nothing to deploy.
    SameAsLive { digest: String },
    /// The image of the run's commit was already in the registry, but it is
    /// not the one that runs (e.g. after a rollback).
    AlreadyBuilt { digest: String },
    /// The run passed and no image reached the registry.
    NoImage,
    /// Nothing conclusive yet.
    Unknown,
}

/// `live` is the deployment that serves now (from the history fetched before
/// the dispatch), `run` and `image` the `workflow_run` and `image_pushed`
/// checks once the run passed.
pub fn explain_no_deployment(live: Option<&Value>, run: &Value, image: &Value) -> NoDeployment {
    if image["code"] == "run_ok_but_no_image" {
        return NoDeployment::NoImage;
    }
    let head = run["details"]["head_sha"].as_str().filter(|h| !h.is_empty());
    let tag = image["details"]["tag"].as_str();
    // Core puts the run's commit as the tag when the image of that commit was
    // already there; otherwise the digest is the newest image of the index.
    let digest = image["details"]["digest"].as_str().filter(|d| !d.is_empty() && image["code"] == "ok" && head.is_some() && tag == head);
    let live_digest = live.and_then(|l| l["image_digest"].as_str()).filter(|d| !d.is_empty());
    let live_commit = live.and_then(|l| l["commit_sha"].as_str()).filter(|c| !c.is_empty());
    match (digest, live_digest) {
        (Some(d), Some(l)) if d.eq_ignore_ascii_case(l) => NoDeployment::SameAsLive { digest: d.to_owned() },
        (Some(d), _) => NoDeployment::AlreadyBuilt { digest: d.to_owned() },
        (None, Some(l)) if head.is_some() && head == live_commit => NoDeployment::SameAsLive { digest: l.to_owned() },
        _ => NoDeployment::Unknown,
    }
}

/// The deployment that serves now: the service's current one, or the newest live.
fn live_of<'a>(service: &Value, items: &'a [Value]) -> Option<&'a Value> {
    let current = service["current_deployment_id"].as_str();
    items
        .iter()
        .find(|d| current.is_some_and(|c| d["id"] == c))
        .or_else(|| items.iter().find(|d| d["status"] == "live"))
}

/// What `POST …/deploy` would do, as far as fdt can tell without calling it.
fn predict(service: &Value, rebuild: bool) -> Value {
    let has_image = !service["current_deployment_id"].is_null() || !service["latest_deployment"].is_null();
    if rebuild {
        json!({"would": "workflow", "reason": "rebuild", "certain": true})
    } else if !has_image {
        json!({"would": "workflow", "reason": "no_image", "certain": true})
    } else {
        json!({
            "would": "redeploy_or_workflow", "reason": null, "certain": false,
            "explain": "Core redespliega la imagen si la cabeza de la rama ya tiene una (image_up_to_date) o lanza el workflow si hay commits nuevos (new_commits); lo decide al lanzar y no hay ruta para preguntarlo sin mutar. --rebuild fuerza el workflow.",
        })
    }
}

pub async fn deploy(ctx: &Ctx, query: Option<String>, rebuild: bool, wait: Wait, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let base = service_path(&org.id, &resolve::id(&service));
    let body = rebuild.then(|| json!({"rebuild": true}));
    if dry {
        let prediction = predict(&service, rebuild);
        ui::info(&match prediction["would"].as_str() {
            Some("workflow") => format!("Lanzaría el workflow de GitHub Actions ({}).", s(&prediction, "reason")),
            _ => "Redesplegaría la imagen de la rama si ya existe, o lanzaría el workflow si hay commits nuevos.".to_owned(),
        });
        return dry_run_with(ctx, "deploy", "POST", &format!("{base}/deploy"), body, json!({"prediction": prediction}));
    }
    let deadline = Instant::now() + wait.timeout;
    let before_items = if wait.wait { deployments(&api, &base).await? } else { vec![] };
    let before: Vec<String> = before_items.iter().map(resolve::id).collect();
    // The latest workflow run before the dispatch, to tell ours apart. Only
    // to explain a build that ends without a deployment: if it cannot be
    // read, fdt waits as before.
    let run_before = if wait.wait { build_checks(&api, &base).await.map(|(run, _)| run) } else { None };
    let mut data = api.call(Method::POST, &format!("{base}/deploy"), body.as_ref()).await?;
    let reason = s(&data, "reason");
    if data["dispatched"] == true {
        ui::info(&format!("Se lanzó el workflow de GitHub Actions ({reason}); el deploy empieza cuando llegue la imagen."));
    } else {
        ui::info(&format!("Redespliegue de la imagen existente ({reason})."));
    }
    if !wait.wait {
        return ctx.out.print(&data, || {
            if data["deployment"].is_null() {
                format!("Sigue el build con: fdt deploys {slug}")
            } else {
                format!("deployment {} · fdt deploys {slug} {}", s(&data["deployment"], "id"), s(&data["deployment"], "id"))
            }
        });
    }
    let id = match data["deployment"]["id"].as_str() {
        Some(id) => id.to_owned(),
        None => {
            // Core does not return a run or deployment id when it dispatches the
            // workflow: the deployment is the one the image push creates. If
            // several appear (parallel deploys), we follow the newest and say so.
            // If the build pushes an image the registry already has (a rebuild
            // that changes nothing), the registry sends no push and Core
            // creates no deployment: the run's outcome and the image check of
            // …/diagnostics tell that apart from a slow build.
            ui::info("Esperando a que el build publique la imagen…");
            let grace = super::poll_unit() * 120;
            let mut polls: u32 = 0;
            let mut run_now: Option<Value> = None;
            let mut image_now = Value::Null;
            let mut run_ok_at: Option<Instant> = None;
            loop {
                let new: Vec<String> = deployments(&api, &base).await?.iter().map(resolve::id).filter(|i| !before.contains(i)).collect();
                if let Some(first) = new.first() {
                    if new.len() > 1 {
                        ui::warn(&format!(
                            "Aparecieron {} deployments nuevos a la vez (¿otro deploy en paralelo?): sigo el más reciente ({first}), que puede no ser el tuyo. Revisa fdt deploys {slug}.",
                            new.len()
                        ));
                        data["ambiguous"] = json!(true);
                        data["candidates"] = json!(new);
                    }
                    break first.clone();
                }
                if let Some(before_run) = &run_before
                    && polls % 4 == 0
                    && let Some((run, image)) = build_checks(&api, &base).await
                {
                    let url = run["details"]["run_url"].as_str().filter(|u| !u.is_empty());
                    let ours = url.is_some() && url != before_run["details"]["run_url"].as_str();
                    if ours {
                        match run["code"].as_str() {
                            Some("failed") | Some("cancelled") => {
                                let how = if run["code"] == "failed" { "falló" } else { "se canceló" };
                                return Err(Failure::new(
                                    Kind::DeployFailed,
                                    format!("El build de GitHub Actions {how} ({}): no llegó imagen y no hay deployment.", url.unwrap_or_default()),
                                )
                                .hint(format!("fdt service diagnose {slug} · el log del run: {}", url.unwrap_or_default()))
                                .details(json!({"workflow_run": run, "image_pushed": image}))
                                .into());
                            }
                            Some("ok") => {
                                run_ok_at.get_or_insert_with(Instant::now);
                            }
                            _ => {}
                        }
                        run_now = Some(run);
                        image_now = image;
                    }
                }
                if let (Some(at), Some(run)) = (run_ok_at, &run_now)
                    && at.elapsed() >= grace
                {
                    let live = live_of(&service, &before_items);
                    match explain_no_deployment(live, run, &image_now) {
                        NoDeployment::SameAsLive { digest } if service["status"] == "paused" => {
                            return Err(Failure::new(
                                Kind::Error,
                                format!(
                                    "El build terminó bien y produjo la misma imagen ({}) que ya tenía el servicio: el registry no avisa de un push que no cambia nada, así que no se creó deployment y {slug} sigue pausado.",
                                    short_digest(&digest)
                                ),
                            )
                            .hint(format!("fdt service resume {slug} --wait"))
                            .details(json!({"workflow_run": run, "image_pushed": image_now}))
                            .into());
                        }
                        NoDeployment::SameAsLive { digest } => {
                            let live = live.cloned().unwrap_or(Value::Null);
                            ui::success(&format!(
                                "Sin cambios: el build produjo la misma imagen ({}) que ya corre en {slug} (deployment {}). Core no crea un deployment si la imagen no cambia; el servicio sigue live: {}",
                                short_digest(&digest),
                                short(&s(&live, "id")),
                                s(&service, "url")
                            ));
                            ui::info(&format!("Para reiniciar el contenedor con esa imagen: fdt service restart {slug} o fdt redeploy {slug} --wait."));
                            data["unchanged"] = json!(true);
                            data["unchanged_reason"] = json!("same_image");
                            data["image_digest"] = json!(digest);
                            data["workflow_run"] = run.clone();
                            data["deployment"] = live;
                            return ctx.out.print(&data, String::new);
                        }
                        NoDeployment::AlreadyBuilt { digest } => {
                            return Err(Failure::new(
                                Kind::Error,
                                format!(
                                    "El build terminó bien, pero la imagen de ese commit ({}) ya estaba en el registry y no es la que corre: el registry no avisa de un push que no cambia nada, así que no se creó deployment.",
                                    short_digest(&digest)
                                ),
                            )
                            .hint(format!("fdt deploy {slug} --wait (sin --rebuild: Core despliega la imagen de la cabeza de la rama) · fdt rollback {slug} --digest {digest} --wait"))
                            .details(json!({"workflow_run": run, "image_pushed": image_now}))
                            .into());
                        }
                        NoDeployment::NoImage => {
                            return Err(Failure::new(
                                Kind::DeployFailed,
                                "El build de GitHub Actions terminó bien pero el registry no recibió ninguna imagen: no hay deployment.",
                            )
                            .hint(format!("fdt service diagnose {slug} (registry_token, image_pushed) · el log del run: {}", s(&run["details"], "run_url")))
                            .details(json!({"workflow_run": run, "image_pushed": image_now}))
                            .into());
                        }
                        NoDeployment::Unknown => {}
                    }
                }
                if Instant::now() > deadline {
                    return Err(wait_image_timeout(&slug, run_before.is_some(), run_now.as_ref(), &image_now));
                }
                polls = polls.wrapping_add(1);
                tokio::time::sleep(poll_every()).await;
            }
        }
    };
    let final_deployment = wait_for(&api, &base, &id, &slug, deadline).await?;
    ui::success(&format!("{slug} está live: {}", s(&service, "url")));
    data["deployment"] = final_deployment;
    ctx.out.print(&data, String::new)
}

fn short_digest(digest: &str) -> String {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    format!("sha256:{}", &hex[..hex.len().min(12)])
}

/// The --timeout error while no deployment appeared, saying what fdt saw of
/// the build instead of guessing.
fn wait_image_timeout(slug: &str, watched: bool, run: Option<&Value>, image: &Value) -> anyhow::Error {
    let seen = match run.and_then(|r| r["code"].as_str()) {
        _ if !watched => "fdt no pudo leer el estado del build (…/diagnostics).".to_owned(),
        None => "Core aún no ha recibido el run del workflow que se lanzó: puede seguir en cola en GitHub Actions, o Actions está deshabilitado en el repo.".to_owned(),
        Some("ok") => "El build terminó bien, pero no llegó ninguna imagen nueva: si la imagen no cambió, el registry no avisa y Core no crea deployment.".to_owned(),
        Some(code) => format!("El build sigue en GitHub Actions ({code}): {}.", run.map(|r| s(&r["details"], "run_url")).unwrap_or_default()),
    };
    Failure::new(
        Kind::Timeout,
        format!("No apareció ningún deployment nuevo antes de --timeout (salida 8: no es un fallo confirmado). {seen} Comprueba el run y la imagen con fdt service diagnose {slug} (workflow_run, image_pushed) y el historial con fdt deploys {slug}."),
    )
    .hint(format!("fdt service diagnose {slug} · fdt deploys {slug}"))
    .details(json!({"workflow_run": run, "image_pushed": image}))
    .into()
}

/// `sha256:` and 64 lowercase hex digits.
pub fn valid_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

#[allow(clippy::too_many_arguments)]
pub async fn rollback(
    ctx: &Ctx,
    query: Option<String>,
    tag: Option<String>,
    digest: Option<String>,
    to: Option<String>,
    wait: Wait,
    dry: bool,
) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let base = service_path(&org.id, &resolve::id(&service));
    let body = match (tag, digest, to) {
        (Some(t), None, None) => json!({"image_tag": t}),
        (None, Some(d), None) => {
            let d = d.trim().to_owned();
            if !valid_digest(&d) {
                return Err(fail(Kind::Usage, format!("--digest «{d}» no es un digest: sha256:<64 hex en minúsculas>.")));
            }
            json!({"image_digest": d})
        }
        (None, None, Some(dep)) => {
            let items = api.get(&format!("{base}/deployments")).await?.as_array().cloned().unwrap_or_default();
            let d = resolve::pick(&items, &dep, &[], "deployment", &format!("fdt deploys {slug}"))?;
            let digest = d["image_digest"].as_str().filter(|x| !x.is_empty()).ok_or_else(|| {
                fail(Kind::Error, format!("El deployment {} no tiene image_digest: usa --tag o --digest.", resolve::id(&d)))
            })?;
            rollback_target_ok(&slug, &d, digest)?;
            json!({"image_digest": digest})
        }
        _ => {
            return Err(ctx.need("--tag, --digest o --to <deployment>", "rollback necesita saber a qué imagen volver"));
        }
    };
    let path = format!("{base}/rollback");
    if dry {
        return dry_run(ctx, "rollback", "POST", &path, Some(body));
    }
    let deadline = Instant::now() + wait.timeout;
    let mut data = api.post(&path, &body).await?;
    ui::info(&format!("Rollback {} en cola.", s(&data, "id")));
    if wait.wait {
        data = wait_for(&api, &base, &resolve::id(&data), &slug, deadline).await?;
        ui::success(&format!("{slug} está live con la imagen anterior."));
    }
    ctx.out.print(&data, || s(&data, "id"))
}

/// `--to` only goes back to an image that served: a deployment that ended
/// live (or was live and got replaced: superseded). Core checks only that the
/// image is in the registry, so a failed one would be accepted and fail again.
fn rollback_target_ok(slug: &str, d: &Value, digest: &str) -> Result<()> {
    let status = s(d, "status");
    if matches!(status.as_str(), "live" | "superseded" | "-") {
        return Ok(());
    }
    let id = resolve::id(d);
    let why = match status.as_str() {
        "queued" | "deploying" => format!("sigue en {status}: no ha terminado"),
        _ => {
            let code = d["error_code"].as_str().map(|c| format!(", {c}")).unwrap_or_default();
            format!("terminó en {status}{code}: su imagen nunca llegó a servir")
        }
    };
    Err(Failure::new(
        Kind::Usage,
        format!(
            "El deployment {} {why}, así que no es un punto seguro al que volver. Elige uno live o superseded (fdt deploys {slug}); si aun así quieres esa imagen, pásala con --digest {digest}.",
            short(&id)
        ),
    )
    .hint(format!("fdt deploys {slug}"))
    .details(json!({"deployment": {"id": id, "status": status, "error_code": d["error_code"], "image_digest": digest}}))
    .into())
}

/// The image `POST …/redeploy` runs again, as Core picks it: the current
/// deployment's, or if none was ever live, the last that arrived. Also the
/// newest image built from the branch when it is a different one (after a
/// rollback, say), so the message can tell.
fn redeploy_image(service: &Value, items: &[Value]) -> (Option<Value>, Option<Value>) {
    let current = live_of(service, items).or_else(|| items.first()).cloned();
    let newest_built = items.iter().find(|d| d["source"] == "registry_push" && matches!(d["status"].as_str(), Some("live" | "superseded")));
    let newer = match (&current, newest_built) {
        (Some(c), Some(n)) if !c["image_digest"].as_str().unwrap_or_default().eq_ignore_ascii_case(n["image_digest"].as_str().unwrap_or_default()) => {
            Some(n.clone())
        }
        _ => None,
    };
    (current, newer)
}

fn image_label(d: &Value) -> String {
    let tag = d["image_tag"].as_str().filter(|t| !t.is_empty()).map(|t| if t.len() == 40 { t[..7].to_owned() } else { t.to_owned() });
    let digest = d["image_digest"].as_str().filter(|x| !x.is_empty()).map(short_digest);
    match (tag, digest) {
        (Some(t), Some(d)) => format!("{t} ({d})"),
        (Some(t), None) => t,
        (None, Some(d)) => d,
        (None, None) => "-".into(),
    }
}

fn image_summary(d: &Value) -> Value {
    json!({
        "deployment_id": d["id"], "source": d["source"], "status": d["status"],
        "image_tag": d["image_tag"], "image_digest": d["image_digest"], "commit_sha": d["commit_sha"],
    })
}

pub async fn redeploy(ctx: &Ctx, query: Option<String>, wait: Wait, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let base = service_path(&org.id, &resolve::id(&service));
    // Only to say which image it runs: without the history, redeploy still goes.
    let items = deployments(&api, &base).await.unwrap_or_default();
    let (current, newer) = redeploy_image(&service, &items);
    if let Some(c) = &current {
        ui::info(&format!(
            "redeploy vuelve a desplegar la imagen que corre ahora, {} (deployment {}, origen {}), sin build y sin mirar la rama.",
            image_label(c),
            short(&s(c, "id")),
            s(c, "source")
        ));
    }
    if let Some(n) = &newer {
        ui::warn(&format!(
            "No es la última imagen construida de la rama ({}){}. Para volver a la última: fdt deploy {slug} --wait.",
            image_label(n),
            if current.as_ref().is_some_and(|c| c["source"] == "rollback") { ": lo que corre viene de un rollback" } else { "" }
        ));
    }
    let extra = json!({
        "image": current.as_ref().map(image_summary),
        "newer_image": newer.as_ref().map(image_summary),
    });
    if dry {
        return dry_run_with(ctx, "redeploy", "POST", &format!("{base}/redeploy"), None, extra);
    }
    let deadline = Instant::now() + wait.timeout;
    let mut data = api.call(Method::POST, &format!("{base}/redeploy"), None).await?;
    ui::info(&format!("Redeploy {} en cola con la imagen {}.", s(&data, "id"), image_label(&data)));
    if wait.wait {
        data = wait_for(&api, &base, &resolve::id(&data), &slug, deadline).await?;
        ui::success(&format!("{slug} está live con {}.", image_label(&data)));
    }
    if let Some(n) = &newer
        && data.is_object()
    {
        data["newer_image"] = image_summary(n);
    }
    ctx.out.print(&data, || s(&data, "id"))
}

pub async fn deploys(
    ctx: &Ctx,
    query: Option<String>,
    deployment: Option<String>,
    limit: usize,
    job: Option<i64>,
    tail: u32,
) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let base = service_path(&org.id, &resolve::id(&service));
    let list = api.get(&format!("{base}/deployments")).await?;
    let items: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let Some(wanted) = deployment else {
        let shown = Value::Array(items.into_iter().take(limit).collect());
        return ctx.out.print(&shown, || {
            let rows: Vec<Vec<String>> = shown
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| vec![
                    s(d, "id").chars().take(8).collect(),
                    ui::status(&s(d, "status")),
                    s(d, "source"),
                    s(d, "image_tag"),
                    s(d, "commit_sha").chars().take(7).collect(),
                    when(d, "created_at"),
                    s(d, "error_code"),
                ])
                .collect();
            ui::table(&["id", "estado", "origen", "tag", "commit", "creado", "error"], &rows)
        });
    };
    let d = resolve::pick(&items, &wanted, &[], "deployment", &format!("fdt deploys {}", s(&service, "slug")))?;
    let did = resolve::id(&d);
    if let Some(job) = job {
        if !(1..=5000).contains(&tail) {
            return Err(fail(Kind::Usage, "--tail debe estar entre 1 y 5000."));
        }
        let log = api
            .get_query(&format!("{base}/deployments/{}/jobs/{job}/logs", enc(&did)), &[("tail".into(), tail.to_string())])
            .await?;
        return ctx.out.print(&log, || log["text"].as_str().unwrap_or_default().to_owned());
    }
    let detail = api.get(&format!("{base}/deployments/{}", enc(&did))).await?;
    ctx.out.print(&detail, || {
        let dep = &detail["deployment"];
        let mut text = fields(&[
            ("deployment", s(dep, "id")),
            ("estado", format!("{} {}", ui::status(&s(dep, "status")), s(dep, "phase"))),
            ("origen", s(dep, "source")),
            ("imagen", format!("{} {}", s(dep, "image_tag"), s(dep, "image_digest"))),
            ("commit", s(dep, "commit_sha")),
            ("error", format!("{} {}", s(dep, "error_code"), s(dep, "error_message"))),
            ("creado", when(dep, "created_at")),
            ("terminó", when(dep, "finished_at")),
        ]);
        if matches!(dep["status"].as_str(), Some("failed" | "cancelled")) {
            text.push_str(&format!(
                "\nlogs del contenedor (si llegó a arrancar): {}",
                s(&failure_commands(&s(&service, "slug"), dep), "logs")
            ));
        }
        match detail["build"].as_object() {
            Some(_) => {
                text.push_str("\n\nBuild:");
                for j in detail["build"]["jobs"].as_array().into_iter().flatten() {
                    text.push_str(&format!(
                        "\n  job {} {} {} · fdt deploys {} {} --job {}",
                        s(j, "id"),
                        s(j, "name"),
                        ui::status(&s(j, "conclusion")),
                        s(&service, "slug"),
                        &did[..did.len().min(8)],
                        s(j, "id")
                    ));
                }
            }
            None => text.push_str(&format!("\n\nSin build: {}", s(&detail, "build_reason"))),
        }
        text
    })
}

pub async fn logs(
    ctx: &Ctx,
    query: Option<String>,
    tail: u32,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
    if !(1..=1000).contains(&tail) {
        return Err(fail(Kind::Usage, "--tail debe estar entre 1 y 1000."));
    }
    if let (Some(a), Some(b)) = (since, until)
        && a >= b
    {
        return Err(fail(Kind::Usage, "--since tiene que ser anterior a --until."));
    }
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let base = service_path(&org.id, &resolve::id(&service));
    if service["status"] == "paused" {
        ui::warn(&format!(
            "{} está pausado (0 contenedores): no hay logs nuevos hasta fdt service resume {}.",
            s(&service, "slug"),
            s(&service, "slug")
        ));
    }
    let mut q = vec![("tail".to_owned(), tail.to_string())];
    let fmt = |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if let Some(t) = since {
        q.push(("since".into(), fmt(t)));
    }
    if let Some(t) = until {
        q.push(("until".into(), fmt(t)));
    }
    let mut data = api.get_query(&format!("{base}/logs"), &q).await?;
    // Core filters by since/until; filtering here too keeps an older Core honest.
    if let Some(lines) = data["lines"].as_array_mut() {
        lines.retain(|l| {
            let at = l["at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
            at.is_none_or(|t| since.is_none_or(|s| t >= s) && until.is_none_or(|u| t <= u))
        });
    }
    ctx.out.print(&data, || {
        let c = ui::Colors::stdout();
        data["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| {
                // stderr is where most programs log, not only errors: no red.
                let text = l["text"].as_str().unwrap_or_default();
                format!("{} {text}", c.gray(&s(l, "at")))
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::{NoDeployment, explain_no_deployment};
    use serde_json::json;

    #[test]
    fn a_build_without_deployment_is_explained_by_the_image_check() {
        let head = "a".repeat(40);
        let live_digest = format!("sha256:{}", "1".repeat(64));
        let other = format!("sha256:{}", "2".repeat(64));
        let live = json!({"id": "dep0", "image_digest": live_digest, "commit_sha": head});
        let run = json!({"code": "ok", "details": {"head_sha": head}});
        let image = |code: &str, tag: &str, digest: &str| json!({"code": code, "details": {"tag": tag, "digest": digest}});
        assert_eq!(
            explain_no_deployment(Some(&live), &run, &image("ok", &head, &live_digest)),
            NoDeployment::SameAsLive { digest: live_digest.clone() }
        );
        assert_eq!(
            explain_no_deployment(Some(&live), &run, &image("ok", &head, &other)),
            NoDeployment::AlreadyBuilt { digest: other.clone() }
        );
        assert_eq!(explain_no_deployment(Some(&live), &run, &image("error", &head, "")), NoDeployment::SameAsLive { digest: live_digest.clone() });
        assert_eq!(explain_no_deployment(Some(&live), &run, &image("run_ok_but_no_image", "", "")), NoDeployment::NoImage);
        // The newest image of another commit says nothing about this run.
        let other_run = json!({"code": "ok", "details": {"head_sha": "b".repeat(40)}});
        assert_eq!(explain_no_deployment(Some(&live), &other_run, &image("ok", &head, &live_digest)), NoDeployment::Unknown);
        assert_eq!(explain_no_deployment(None, &run, &json!(null)), NoDeployment::Unknown);
    }

    #[test]
    fn digests_are_sha256_and_64_hex() {
        assert!(super::valid_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(!super::valid_digest("sha256:x"));
        assert!(!super::valid_digest(&"a".repeat(64)));
        assert!(!super::valid_digest(&format!("sha256:{}", "A".repeat(64))));
    }
}
