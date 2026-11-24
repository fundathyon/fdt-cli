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

async fn deployment_ids(api: &Api, base: &str) -> Result<Vec<String>> {
    Ok(api
        .get(&format!("{base}/deployments"))
        .await?
        .as_array()
        .into_iter()
        .flatten()
        .map(resolve::id)
        .collect())
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
            ui::info(&format!("deploy {}: {status}{}", &id[..id.len().min(8)], if phase == "-" { String::new() } else { format!(" ({phase})") }));
            last = now;
        }
        match status.as_str() {
            "live" => return Ok(d.clone()),
            "superseded" => {
                ui::warn("Otro deploy más nuevo reemplazó a este antes de terminar.");
                return Ok(d.clone());
            }
            "failed" | "cancelled" => {
                return Err(Failure::new(
                    Kind::DeployFailed,
                    format!(
                        "El deploy {status}: {} {}",
                        s(d, "error_code"),
                        d["error_message"].as_str().unwrap_or_default()
                    ),
                )
                .hint(format!("fdt deploys {slug} {id} · fdt service diagnose {slug} · fdt logs {slug}"))
                .details(json!({"deployment": d}))
                .into());
            }
            _ => {}
        }
        if Instant::now() > deadline {
            return Err(Failure::new(Kind::Timeout, format!("El deploy sigue en {status} al acabar --timeout (salida 8; no falló, aún no termina)."))
                .hint(format!("fdt deploys {slug} {id}"))
                .details(json!({"deployment": d}))
                .into());
        }
        tokio::time::sleep(poll_every()).await;
    }
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
    let before = if wait.wait { deployment_ids(&api, &base).await? } else { vec![] };
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
            ui::info("Esperando a que el build publique la imagen…");
            loop {
                let new: Vec<String> = deployment_ids(&api, &base).await?.into_iter().filter(|i| !before.contains(i)).collect();
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
                if Instant::now() > deadline {
                    return Err(Failure::new(Kind::Timeout, "El build no publicó ninguna imagen antes de --timeout.")
                        .hint(format!("fdt service diagnose {slug} · fdt deploys {slug}"))
                        .into());
                }
                tokio::time::sleep(poll_every()).await;
            }
        }
    };
    let final_deployment = wait_for(&api, &base, &id, &slug, deadline).await?;
    ui::success(&format!("{slug} está live: {}", s(&service, "url")));
    data["deployment"] = final_deployment;
    ctx.out.print(&data, String::new)
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
        (None, Some(d), None) => json!({"image_digest": d}),
        (None, None, Some(dep)) => {
            let items = api.get(&format!("{base}/deployments")).await?.as_array().cloned().unwrap_or_default();
            let d = resolve::pick(&items, &dep, &[], "deployment", &format!("fdt deploys {slug}"))?;
            let digest = d["image_digest"].as_str().filter(|x| !x.is_empty()).ok_or_else(|| {
                fail(Kind::Error, format!("El deployment {} no tiene image_digest: usa --tag o --digest.", resolve::id(&d)))
            })?;
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

pub async fn redeploy(ctx: &Ctx, query: Option<String>, wait: Wait, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, query.as_deref()).await?;
    let slug = s(&service, "slug");
    let base = service_path(&org.id, &resolve::id(&service));
    if dry {
        return dry_run(ctx, "redeploy", "POST", &format!("{base}/redeploy"), None);
    }
    let deadline = Instant::now() + wait.timeout;
    let mut data = api.call(Method::POST, &format!("{base}/redeploy"), None).await?;
    ui::info(&format!("Redeploy {} en cola.", s(&data, "id")));
    if wait.wait {
        data = wait_for(&api, &base, &resolve::id(&data), &slug, deadline).await?;
        ui::success(&format!("{slug} está live."));
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
                let text = s(l, "text");
                let text = if l["stream"] == "stderr" { c.red(&text) } else { text };
                format!("{} {text}", c.gray(&s(l, "at")))
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}
