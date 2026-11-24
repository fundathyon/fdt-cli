//! `fdt db list|show|create|delete|url|credentials|link|unlink|connect`.
//! Passwords only leave fdt through `db url` and `db credentials`, which exist
//! to print them; `fdt env set KEY --from-db <base>` wires a base into a
//! service without printing anything.

use super::databases_path;
use crate::api::{Api, enc};
use crate::config::OrgRef;
use crate::context::{Ctx, dry_run};
use crate::failure::{Failure, Kind, fail};
use crate::output::{fields, s, when};
use crate::{resolve, secrets, ui};
use anyhow::Result;
use serde_json::{Value, json};
use std::time::Instant;

fn rows(dbs: &[Value]) -> Vec<Vec<String>> {
    dbs.iter()
        .map(|d| vec![s(d, "name"), s(d, "engine"), ui::status(&s(d, "status")), format!("{}:{}", s(d, "internal_host"), s(d, "port")), s(d, "public_host"), s(d, "id")])
        .collect()
}

const HEADERS: [&str; 6] = ["base", "motor", "estado", "host interno", "host público", "id"];

fn detail(d: &Value) -> String {
    fields(&[
        ("base", s(d, "name")),
        ("id", s(d, "id")),
        ("motor", s(d, "engine")),
        ("estado", ui::status(&s(d, "status"))),
        ("base de datos", s(d, "db_name")),
        ("usuario", s(d, "username")),
        ("host interno", format!("{}:{}", s(d, "internal_host"), s(d, "port"))),
        ("host público", s(d, "public_host")),
        ("cadena interna", s(d, "internal_connection_string")),
        ("proyectos", d["project_ids"].as_array().map(|a| a.len().to_string()).unwrap_or_else(|| "0".into())),
        ("error", s(d, "error_message")),
        ("creada", when(d, "created_at")),
    ])
}

pub async fn list(ctx: &Ctx, project: Option<String>) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    match ctx.project_opt(&api, &org, project.as_deref()).await? {
        Some(p) => {
            let data = api.get(&format!("/organizations/{}/projects/{}/databases", enc(&org.id), enc(&resolve::id(&p)))).await?;
            ctx.out.print(&data, || ui::table(&HEADERS, &rows(data.as_array().map_or(&[], |v| v))))
        }
        None => {
            let data = resolve::databases_view(&api, &org.id).await?;
            ctx.out.print(&data, || {
                let limit = match data["limit"].as_i64() {
                    Some(-1) => "sin límite".to_owned(),
                    _ => s(&data, "limit"),
                };
                format!(
                    "{}\n\n{} de {limit} bases",
                    ui::table(&HEADERS, &rows(data["databases"].as_array().map_or(&[], |v| v))),
                    s(&data, "used")
                )
            })
        }
    }
}

pub async fn show(ctx: &Ctx, query: String) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let db = resolve::database(&api, &org.id, &query).await?;
    let data = api.get(&format!("{}/{}", databases_path(&org.id), enc(&resolve::id(&db)))).await?;
    ctx.out.print(&data, || detail(&data))
}

pub struct CreateArgs {
    pub name: String,
    pub engine: String,
    pub db_name: Option<String>,
    pub db_user: Option<String>,
    pub project: Option<String>,
    pub if_not_exists: bool,
    pub wait: bool,
    pub timeout: std::time::Duration,
    pub dry_run: bool,
}

pub async fn create(ctx: &Ctx, a: CreateArgs) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let name = a.name.trim().to_lowercase();
    let view = resolve::databases_view(&api, &org.id).await?;
    let mut data = None;
    if a.if_not_exists
        && let Some(existing) = view["databases"].as_array().into_iter().flatten().find(|d| d["name"].as_str() == Some(name.as_str()))
    {
        ui::info(&format!("La base «{name}» ya existía."));
        data = Some(existing.clone());
    }
    if data.is_none() {
        check_engine(&a.engine, &view)?;
    }
    let mut data = match data {
        Some(d) => d,
        None => {
            let mut body = json!({"engine": a.engine, "name": name});
            if let Some(v) = &a.db_name {
                body["db_name"] = json!(v);
            }
            if let Some(v) = &a.db_user {
                body["db_user"] = json!(v);
            }
            if let Some(p) = &a.project {
                body["project_id"] = json!(resolve::id(&resolve::project(&api, &org.id, p).await?));
            }
            if a.dry_run {
                return dry_run(ctx, "db create", "POST", &databases_path(&org.id), Some(body));
            }
            let created = api
                .post(&databases_path(&org.id), &body)
                .await
                .map_err(|e| crate::api::explain_scope(e, "crear bases de datos"))?;
            ui::success(&format!("Base «{}» creada (provisioning).", s(&created, "name")));
            created
        }
    };
    if a.wait {
        let deadline = Instant::now() + a.timeout;
        let path = format!("{}/{}", databases_path(&org.id), enc(&resolve::id(&data)));
        loop {
            match data["status"].as_str().unwrap_or_default() {
                "ready" => break,
                "failed" => {
                    return Err(Failure::new(Kind::Error, format!("La base falló al crearse: {}", s(&data, "error_message")))
                        .hint(format!("fdt db show {name}"))
                        .into());
                }
                _ => {}
            }
            if Instant::now() > deadline {
                return Err(Failure::new(Kind::Timeout, "La base no estuvo lista antes de --timeout.").hint(format!("fdt db show {name}")).into());
            }
            tokio::time::sleep(super::poll_unit() * 3).await;
            data = api.get(&path).await?;
        }
        ui::success(&format!("Base «{name}» lista."));
    }
    ctx.out.print(&data, || detail(&data))
}

/// The engines the org can create (`engines` of `GET …/deploy/databases`,
/// Core's DEPLOY_DB_ENGINES). Without that list, Core decides.
fn check_engine(engine: &str, view: &Value) -> Result<()> {
    let Some(engines) = view["engines"].as_array() else { return Ok(()) };
    let engines: Vec<&str> = engines.iter().filter_map(Value::as_str).collect();
    if engines.contains(&engine) {
        return Ok(());
    }
    let offered = if engines.is_empty() { "ninguno".to_owned() } else { engines.join(", ") };
    Err(Failure::new(Kind::Usage, format!("La organización no ofrece el motor {engine}: ofrece {offered}."))
        .hint(match engines.first() {
            Some(e) => format!("--engine {e}"),
            None => "la creación de bases no está activa: avisa a quien opera la plataforma".to_owned(),
        })
        .details(json!({"engines": engines}))
        .into())
}

pub async fn delete(ctx: &Ctx, query: String, confirm: Option<String>, dry: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let db = resolve::database(&api, &org.id, &query).await?;
    let name = s(&db, "name");
    let path = format!("{}/{}", databases_path(&org.id), enc(&resolve::id(&db)));
    // Also in a dry run: it would pass where the real one is refused otherwise.
    ctx.confirm(&name, confirm.as_deref(), "borrar la base de datos")?;
    if dry {
        return dry_run(ctx, "db delete", "DELETE", &path, None);
    }
    let data = api.delete(&path).await?;
    ui::success(&format!("Base «{name}» en borrado (deleting)."));
    ctx.out.print(&data, String::new)
}

async fn creds(api: &Api, org: &OrgRef, query: &str) -> Result<(Value, Value)> {
    let db = resolve::database(api, &org.id, query).await?;
    let data = api.get(&format!("{}/{}/credentials", databases_path(&org.id), enc(&resolve::id(&db)))).await?;
    Ok((db, data))
}

/// The full connection string (with password) of a base.
pub async fn connection_string(api: &Api, org: &OrgRef, query: &str, public: bool) -> Result<String> {
    let (db, c) = creds(api, org, query).await?;
    let key = if public { "connection_string" } else { "internal_connection_string" };
    c[key].as_str().filter(|v| !v.is_empty()).map(str::to_owned).ok_or_else(|| {
        if public {
            fail(Kind::Error, format!("La base «{}» no tiene acceso público: usa la cadena interna (sin --public).", s(&db, "name")))
        } else {
            fail(Kind::Api, "La API no devolvió internal_connection_string.")
        }
    })
}

pub async fn url(ctx: &Ctx, query: String, public: bool, reveal: bool) -> Result<()> {
    if !reveal {
        return Err(Failure::new(
            Kind::MissingInput,
            "La URL lleva la contraseña: fdt db url solo la imprime con --reveal. Para pasarla a un servicio sin imprimirla usa fdt env set DATABASE_URL --from-db <base> --service <servicio>.",
        )
        .hint(format!("fdt db url {query} --reveal"))
        .into());
    }
    let (api, org) = super::org_api(ctx).await?;
    let url = connection_string(&api, &org, &query, public).await?;
    if ctx.json_flag {
        crate::output::print_json(&json!({"url": url, "public": public}))
    } else {
        println!("{url}");
        Ok(())
    }
}

/// Hides the password in a credentials answer.
fn mask_credentials(data: &mut Value) {
    let password = data["password"].as_str().unwrap_or_default().to_owned();
    if !password.is_empty() {
        data["password"] = json!(secrets::MASK);
    }
    for key in ["internal_connection_string", "connection_string"] {
        if let Some(text) = data[key].as_str() {
            let mut masked = crate::secrets::redact(text);
            if !password.is_empty() {
                masked = masked.replace(&password, secrets::MASK);
            }
            data[key] = json!(masked);
        }
    }
}

pub async fn credentials(ctx: &Ctx, query: String, reveal: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let data = if reveal {
        creds(&api, &org, &query).await?.1
    } else {
        // Without --reveal the password is not shown, so it is not fetched
        // either: the base itself carries the rest. A read token, which Core
        // keeps away from …/credentials (403 cli.token_level), sees it too.
        let db = resolve::database(&api, &org.id, &query).await?;
        let mut data = json!({"password": secrets::MASK});
        for key in ["username", "db_name", "port", "internal_host", "public_host", "public_access", "internal_connection_string", "connection_string"] {
            if !db[key].is_null() {
                data[key] = db[key].clone();
            }
        }
        mask_credentials(&mut data);
        data["revealed"] = json!(false);
        ui::info("Contraseña oculta: añade --reveal para verla.");
        data
    };
    ctx.out.print(&data, || {
        fields(&[
            ("usuario", s(&data, "username")),
            ("contraseña", s(&data, "password")),
            ("base de datos", s(&data, "db_name")),
            ("host interno", format!("{}:{}", s(&data, "internal_host"), s(&data, "port"))),
            ("host público", s(&data, "public_host")),
            ("cadena interna", s(&data, "internal_connection_string")),
            ("cadena pública", s(&data, "connection_string")),
        ])
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn set_credentials(
    ctx: &Ctx,
    query: String,
    user: Option<String>,
    password_stdin: bool,
    password_from_env: Option<String>,
    confirm: Option<String>,
    dry: bool,
) -> Result<()> {
    let password = if password_stdin {
        let mut raw = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)?;
        Some(raw.trim_end_matches(['\n', '\r']).to_owned())
    } else if let Some(var) = &password_from_env {
        Some(std::env::var(var).map_err(|_| ctx.need(&format!("la variable de entorno {var}"), "--password-from-env la lee de tu entorno"))?)
    } else {
        None
    };
    if user.is_none() && password.is_none() {
        return Err(fail(Kind::Usage, "Indica --user y/o la contraseña (--password-stdin o --password-from-env)."));
    }
    let (api, org) = super::org_api(ctx).await?;
    let db = resolve::database(&api, &org.id, &query).await?;
    let name = s(&db, "name");
    let path = format!("{}/{}/credentials", databases_path(&org.id), enc(&resolve::id(&db)));
    let mut body = json!({});
    if let Some(u) = &user {
        body["db_user"] = json!(u);
    }
    if password.is_some() {
        body["password"] = json!(secrets::MASK);
    }
    ui::warn("Las apps que usan la base dejan de conectar hasta que actualices su DATABASE_URL (fdt env set DATABASE_URL --from-db <base> --service <servicio>).");
    ctx.confirm(&name, confirm.as_deref(), "cambiar las credenciales de la base")?;
    if dry {
        return dry_run(ctx, "db set-credentials", "PATCH", &path, Some(body));
    }
    if let Some(p) = password {
        body["password"] = json!(p);
    }
    let data = api.patch(&path, &body).await?;
    ui::success(&format!("Credenciales de «{name}» cambiadas."));
    ctx.out.print(&data, || detail(&data))
}

pub async fn link(ctx: &Ctx, query: String, project: Option<String>, dry: bool, on: bool) -> Result<()> {
    let (api, org) = super::org_api(ctx).await?;
    let db = resolve::database(&api, &org.id, &query).await?;
    let p = ctx.project(&api, &org, project.as_deref()).await?;
    let base = format!("/organizations/{}/projects/{}/databases", enc(&org.id), enc(&resolve::id(&p)));
    let (method, path, body) = if on {
        ("POST", base.clone(), Some(json!({"database_id": resolve::id(&db)})))
    } else {
        ("DELETE", format!("{base}/{}", enc(&resolve::id(&db))), None)
    };
    if dry {
        return dry_run(ctx, if on { "db link" } else { "db unlink" }, method, &path, body);
    }
    let data = if on { api.post(&path, body.as_ref().unwrap()).await? } else { api.delete(&path).await? };
    ui::success(&format!(
        "Base «{}» {} proyecto «{}».",
        s(&db, "name"),
        if on { "puesta en el" } else { "quitada del" },
        s(&p, "name")
    ));
    let out = if data.is_null() { json!({"ok": true}) } else { data };
    ctx.out.print(&out, String::new)
}

pub async fn connect(ctx: &Ctx, query: String) -> Result<()> {
    if !ui::tty() {
        return Err(fail(Kind::Usage, "fdt db connect abre un cliente interactivo: necesita una terminal. Para scripts usa fdt db url --public."));
    }
    let (api, org) = super::org_api(ctx).await?;
    let (db, c) = creds(&api, &org, &query).await?;
    let public = c["public_host"].as_str().filter(|h| !h.is_empty()).ok_or_else(|| {
        fail(Kind::Error, format!("La base «{}» no tiene acceso público: no se puede conectar desde tu máquina.", s(&db, "name")))
    })?;
    let mut command = match db["engine"].as_str().unwrap_or_default() {
        "postgres" => {
            // The password goes through the environment, never argv.
            let mut cmd = std::process::Command::new("psql");
            cmd.args(["-h", public, "-p", &s(&c, "port"), "-U", &s(&c, "username"), "-d", &s(&c, "db_name")])
                .env("PGPASSWORD", s(&c, "password"))
                .env("PGSSLMODE", "require");
            cmd
        }
        "mongo" => {
            let mut cmd = std::process::Command::new("mongosh");
            cmd.arg(s(&c, "connection_string"));
            cmd
        }
        other => return Err(fail(Kind::Error, format!("No sé conectar a una base {other}."))),
    };
    let status = command.status().map_err(|e| {
        fail(Kind::Error, format!("No se pudo ejecutar {:?}: {e}. ¿Está instalado?", command.get_program()))
    })?;
    if !status.success() {
        return Err(fail(Kind::Error, format!("El cliente terminó con {status}.")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn engines_are_checked_against_what_the_org_offers() {
        let view = serde_json::json!({"engines": ["postgres"]});
        assert!(super::check_engine("postgres", &view).is_ok());
        assert!(super::check_engine("mongo", &view).is_err());
        assert!(super::check_engine("mongo", &serde_json::json!({})).is_ok(), "no list: Core decides");
    }
}
