//! `fdt env list|set|unset|import|pull`. The API only has `PUT …/env` with the
//! whole set, so every change reads, merges and writes. An existing secret is
//! sent without `value` and keeps the one stored in Core.

use super::service_path;
use crate::api::Api;
use crate::config::OrgRef;
use crate::context::{Ctx, dry_run_with};
use crate::envfile::{self, Diff, Var};
use crate::failure::{Kind, fail};
use crate::output::s;
use crate::{resolve, ui};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::io::Read as _;
use std::path::PathBuf;

pub const MASK: &str = "••••••";

struct Target {
    api: Api,
    org: OrgRef,
    service: Value,
    path: String,
}

async fn target(ctx: &Ctx, service: Option<&str>) -> Result<Target> {
    let (api, org) = super::org_api(ctx).await?;
    let service = ctx.service(&api, &org, service).await?;
    let path = format!("{}/env", service_path(&org.id, &resolve::id(&service)));
    Ok(Target { api, org, service, path })
}

fn table(vars: &Value) -> String {
    let rows: Vec<Vec<String>> = vars
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            let value = if v["is_secret"] == true { MASK.to_owned() } else { v["value"].as_str().unwrap_or("").to_owned() };
            vec![s(v, "key"), value, if v["is_secret"] == true { "sí".into() } else { "no".into() }]
        })
        .collect();
    ui::table(&["variable", "valor", "secreta"], &rows)
}

/// Secret values never leave fdt: they come back without `value` from the API,
/// and anything else marked secret is masked here too.
fn masked(vars: &Value) -> Value {
    let mut vars = vars.clone();
    for v in vars.as_array_mut().into_iter().flatten() {
        if v["is_secret"] == true && v.get("value").is_some() {
            v["value"] = json!(MASK);
        }
    }
    vars
}

pub async fn list(ctx: &Ctx, service: Option<String>) -> Result<()> {
    let t = target(ctx, service.as_deref()).await?;
    let vars = masked(&t.api.get(&t.path).await?);
    ctx.out.print(&vars, || table(&vars))
}

/// Whether Core would restart the service for a change: it creates an
/// env_change deployment when something changed and the service has an image,
/// unless it is paused (then it saves without deploying). See Core's PutEnv.
fn would_redeploy(service: &Value, diff: &Diff) -> (bool, Option<&'static str>) {
    if diff.is_empty() {
        return (false, Some("no_changes"));
    }
    if service["status"] == "paused" {
        return (false, Some("service_paused"));
    }
    let has_image = !service["current_deployment_id"].is_null() || !service["latest_deployment"].is_null();
    if has_image { (true, None) } else { (false, Some("no_image")) }
}

/// Reads, merges, shows or writes. `argv_keys` are values that came on the command line.
#[allow(clippy::too_many_arguments)]
async fn apply(
    ctx: &Ctx,
    t: &Target,
    set: &[(String, String, bool)],
    unset: &[String],
    replace: bool,
    dry: bool,
    action: &str,
    argv_keys: &[String],
) -> Result<()> {
    let current = envfile::from_api(&t.api.get(&t.path).await?);
    let (vars, diff) = envfile::merge(&current, set, unset, replace);
    let leaked: Vec<&String> = argv_keys
        .iter()
        .filter(|k| vars.iter().any(|v| &v.key == *k && v.is_secret))
        .collect();
    if !leaked.is_empty() {
        ui::warn(&format!(
            "El valor secreto de {} vino en la línea de comandos: queda en el historial del shell. Usa --stdin, --from-env o --from-file.",
            leaked.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let (redeploys, skipped) = would_redeploy(&t.service, &diff);
    if dry {
        ui::info(&summary(&diff));
        if redeploys {
            ui::warn(&format!("Aplicarlo reiniciaría {} con las variables nuevas (deployment env_change).", s(&t.service, "slug")));
        }
        return dry_run_with(
            ctx,
            action,
            "PUT",
            &t.path,
            Some(envfile::masked_body(&vars)),
            json!({"diff": diff, "redeploys": redeploys, "redeploy_skipped_reason": skipped}),
        );
    }
    if diff.is_empty() {
        ui::info("Sin cambios: no se escribió nada.");
        return ctx.out.print(&json!({"changed": false, "diff": diff, "redeploys": false}), String::new);
    }
    let result = t.api.put(&t.path, &envfile::body(&vars)).await?;
    ui::success(&format!("{} en {}.", summary(&diff), s(&t.service, "slug")));
    let redeployed = !result["deployment"].is_null();
    if redeployed {
        ui::info(&format!(
            "El servicio se reinicia con las variables nuevas (deployment {}; fdt deploys {} {}).",
            s(&result["deployment"], "id"),
            s(&t.service, "slug"),
            s(&result["deployment"], "id")
        ));
    } else if let Some(reason) = result["deployment_skipped_reason"].as_str() {
        ui::info(&format!("Guardadas sin reiniciar ({reason}): se aplican al reanudar o en el próximo deploy."));
    }
    let out = json!({
        "changed": true, "diff": diff, "redeploys": redeployed,
        "deployment": result["deployment"],
        "deployment_skipped_reason": result["deployment_skipped_reason"],
        "vars": masked(&result["vars"]),
    });
    ctx.out.print(&out, || table(&out["vars"]))
}

fn summary(diff: &Diff) -> String {
    format!(
        "Variables: {} nuevas, {} cambiadas, {} quitadas",
        diff.added.len(),
        diff.changed.len(),
        diff.removed.len()
    )
}

pub struct SetArgs {
    pub assignments: Vec<String>,
    pub service: Option<String>,
    pub secret: bool,
    pub from_db: Option<String>,
    pub from_env: Option<String>,
    pub from_file: Option<PathBuf>,
    pub public: bool,
    pub stdin: bool,
    pub dry_run: bool,
}

fn check_key(key: &str) -> Result<()> {
    if envfile::valid_key(key) {
        Ok(())
    } else {
        Err(fail(Kind::Usage, format!("Nombre de variable inválido: «{key}» (letras, dígitos y _)")))
    }
}

pub async fn set(ctx: &Ctx, a: SetArgs) -> Result<()> {
    let t = target(ctx, a.service.as_deref()).await?;
    let mut set = Vec::new();
    let mut argv_keys = Vec::new();
    let single = a.from_db.is_some() || a.stdin || a.from_env.is_some() || a.from_file.is_some();
    if single {
        let [key] = a.assignments.as_slice() else {
            return Err(fail(Kind::Usage, "Con --from-db, --from-env, --from-file o --stdin se pasa una sola KEY."));
        };
        if key.contains('=') {
            return Err(fail(Kind::Usage, "Con --from-db, --from-env, --from-file o --stdin pasa solo KEY, sin =VALOR."));
        }
        check_key(key)?;
        let value = if let Some(db) = &a.from_db {
            super::db::connection_string(&t.api, &t.org, db, a.public).await?
        } else if let Some(var) = &a.from_env {
            std::env::var(var).map_err(|_| {
                ctx.need(&format!("la variable de entorno {var}"), "--from-env lee el valor de tu entorno y no está definida")
            })?
        } else if let Some(file) = &a.from_file {
            std::fs::read_to_string(file)
                .with_context(|| format!("no se pudo leer {}", file.display()))?
                .trim_end_matches(['\n', '\r'])
                .to_owned()
        } else {
            let mut raw = String::new();
            std::io::stdin().read_to_string(&mut raw).context("no se pudo leer stdin")?;
            raw.trim_end_matches(['\n', '\r']).to_owned()
        };
        set.push((key.clone(), value, a.secret || a.from_db.is_some()));
    } else {
        for item in &a.assignments {
            match item.split_once('=') {
                Some((k, v)) => {
                    check_key(k)?;
                    set.push((k.to_owned(), v.to_owned(), a.secret));
                    argv_keys.push(k.to_owned());
                }
                None if ctx.interactive => {
                    check_key(item)?;
                    let value = ui::secret(&format!("Valor de {item}"))?;
                    set.push((item.clone(), value, a.secret));
                }
                None => {
                    return Err(ctx.need(
                        &format!("el valor de {item} (KEY=VALUE, --stdin, --from-env, --from-file o --from-db)"),
                        "sin terminal no se puede preguntar",
                    ));
                }
            }
        }
    }
    apply(ctx, &t, &set, &[], false, a.dry_run, "env set", &argv_keys).await
}

pub async fn unset(ctx: &Ctx, keys: Vec<String>, service: Option<String>, dry: bool) -> Result<()> {
    let t = target(ctx, service.as_deref()).await?;
    apply(ctx, &t, &[], &keys, false, dry, "env unset", &[]).await
}

pub async fn import(
    ctx: &Ctx,
    file: PathBuf,
    service: Option<String>,
    secret: bool,
    secret_keys: Vec<String>,
    replace: bool,
    dry: bool,
) -> Result<()> {
    let text = if file.as_os_str() == "-" {
        let mut raw = String::new();
        std::io::stdin().read_to_string(&mut raw)?;
        raw
    } else {
        std::fs::read_to_string(&file).with_context(|| format!("no se pudo leer {}", file.display()))?
    };
    let parsed = envfile::parse(&text)?;
    let set: Vec<(String, String, bool)> = parsed
        .into_iter()
        .map(|(k, v)| {
            let is_secret = secret || secret_keys.contains(&k);
            (k, v, is_secret)
        })
        .collect();
    let t = target(ctx, service.as_deref()).await?;
    apply(ctx, &t, &set, &[], replace, dry, "env import", &[]).await
}

pub async fn pull(ctx: &Ctx, service: Option<String>, output: Option<PathBuf>) -> Result<()> {
    let t = target(ctx, service.as_deref()).await?;
    let vars: Vec<Var> = envfile::from_api(&t.api.get(&t.path).await?);
    let mut text = format!("# fdt env pull · {} · {}\n", s(&t.service, "slug"), t.org.name);
    let mut secrets = Vec::new();
    for v in &vars {
        if v.is_secret {
            text.push_str(&format!("# {}=(secreta: la API no devuelve su valor)\n", v.key));
            secrets.push(v.key.clone());
        } else {
            text.push_str(&envfile::line(&v.key, v.value.as_deref().unwrap_or("")));
            text.push('\n');
        }
    }
    let plain: serde_json::Map<String, Value> = vars
        .iter()
        .filter(|v| !v.is_secret)
        .map(|v| (v.key.clone(), json!(v.value)))
        .collect();
    match output {
        Some(path) => {
            std::fs::write(&path, &text).with_context(|| format!("no se pudo escribir {}", path.display()))?;
            ui::success(&format!("{} variables en {} ({} secretas sin valor).", plain.len(), path.display(), secrets.len()));
            ctx.out.print(&json!({"path": path, "vars": plain, "secrets": secrets}), String::new)
        }
        None if ctx.json_flag => crate::output::print_json(&json!({"vars": plain, "secrets": secrets})),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}
