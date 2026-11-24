//! `login` (device flow or a given token), `logout`, `whoami` and `token`.

use crate::api::{Api, api_error, enc};
use crate::config::{Credential, DEFAULT_SCOPE, OrgRef, credential_key};
use crate::context::{Ctx, dry_run};
use crate::failure::{Failure, Kind, fail, fail_hint};
use crate::output::{fields, s, when};
use crate::{resolve, secrets, ui};
use anyhow::{Context as _, Result};
use reqwest::Method;
use serde_json::{Value, json};
use std::io::Read as _;
use std::time::Instant;

fn client_name() -> String {
    format!("fdt {}", crate::VERSION)
}

fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Saves a token and what we know of it; returns where it went.
async fn save_token(ctx: &mut Ctx, token: &str, who: Option<&Value>) -> Result<(secrets::Store, String)> {
    let api_url = ctx.api_url()?;
    let info = who.map(|w| &w["token"]);
    let kind = info.and_then(|t| t["kind"].as_str()).map(str::to_owned);
    let org_id = info
        .and_then(|t| t["organization_id"].as_str())
        .filter(|_| kind.as_deref() == Some("scoped"))
        .map(str::to_owned);
    let scope = org_id.clone().unwrap_or_else(|| DEFAULT_SCOPE.to_owned());
    let key = credential_key(&api_url, &scope);
    let store = secrets::save(&ctx.home, &key, token)?;
    ctx.config.credentials.insert(
        key,
        Credential {
            store,
            token_id: info.and_then(|t| t["id"].as_str()).map(str::to_owned),
            kind,
            user: who.and_then(|w| w["user"]["email"].as_str()).map(str::to_owned),
            organization_id: org_id.clone(),
            expires_at: info.and_then(|t| t["expires_at"].as_str()).map(str::to_owned),
        },
    );
    if ctx.api_url_flag.is_some() {
        ctx.config.api_url = Some(api_url);
    }
    // A scoped token only works in its org: make it the active one if there is none.
    if let Some(id) = org_id
        && ctx.config.org.is_none()
    {
        let api = Api::new(&ctx.api_url()?, Some(token.to_owned()))?;
        let name = resolve::orgs(&api)
            .await
            .ok()
            .and_then(|orgs| orgs.into_iter().find(|o| o["id"].as_str() == Some(id.as_str())))
            .and_then(|o| o["name"].as_str().map(str::to_owned))
            .unwrap_or_else(|| id.clone());
        ctx.config.org = Some(OrgRef { id, name });
    }
    ctx.config.save(&ctx.home)?;
    Ok((store, scope))
}

pub async fn login(ctx: &mut Ctx, token: Option<String>, with_token: bool, no_browser: bool) -> Result<()> {
    let token = if with_token {
        let mut raw = String::new();
        std::io::stdin().read_to_string(&mut raw).context("no se pudo leer el token de stdin")?;
        Some(raw)
    } else {
        token
    };
    if let Some(token) = token {
        return login_with_token(ctx, token.trim().to_owned()).await;
    }
    if ctx.no_input {
        return Err(Failure::new(
            Kind::MissingInput,
            "El login por dispositivo espera a que apruebes en el navegador; con --no-input usa un token.",
        )
        .hint("fdt login --with-token < archivo, o exporta FDT_TOKEN")
        .into());
    }
    device_login(ctx, no_browser).await
}

async fn login_with_token(ctx: &mut Ctx, token: String) -> Result<()> {
    if token.is_empty() {
        return Err(fail(Kind::Usage, "El token está vacío."));
    }
    if !token.starts_with("fdt_") {
        ui::warn("El token no empieza por fdt_: se prueba igual (p. ej. un JWT del panel para pruebas) y solo se guarda si la API lo acepta.");
    }
    let api = Api::new(&ctx.api_url()?, Some(token.clone()))?;
    let (who, warning) = match api.get("/cli/whoami").await {
        Ok(who) => (Some(who), None),
        Err(e) if crate::failure::failure_of(&e).is_some_and(|f| f.kind == Kind::NotFound) => (
            None,
            Some("La API aún no tiene /cli/whoami: el token se guardó sin comprobarlo.".to_owned()),
        ),
        Err(e) => return Err(e.context("el token no sirve; no se guardó")),
    };
    let (store, scope) = save_token(ctx, &token, who.as_ref()).await?;
    if let Some(w) = &warning {
        ui::warn(w);
    }
    let data = json!({
        "ok": true, "api_url": ctx.api_url()?, "store": store, "scope": scope,
        "user": who.as_ref().map(|w| w["user"].clone()),
        "token": who.as_ref().map(|w| w["token"].clone()),
        "warning": warning,
    });
    ui::success(&format!("Sesión guardada en el {}.", store.label()));
    ctx.out.print(&data, || {
        who.as_ref()
            .map(|w| format!("Hola, {}.", s(&w["user"], "email")))
            .unwrap_or_default()
    })
}

async fn device_login(ctx: &mut Ctx, no_browser: bool) -> Result<()> {
    let anon = ctx.anonymous()?;
    let code = anon
        .post("/cli/device/code", &json!({"client_name": client_name(), "hostname": hostname()}))
        .await
        .context("no se pudo iniciar el login por dispositivo")?;
    let device_code = code["device_code"]
        .as_str()
        .ok_or_else(|| fail(Kind::Api, "La API no devolvió device_code."))?
        .to_owned();
    let user_code = s(&code, "user_code");
    let uri = s(&code, "verification_uri");
    let complete = code["verification_uri_complete"].as_str().unwrap_or(&uri).to_owned();
    let mut interval = code["interval"].as_u64().unwrap_or(5).max(1);
    let expires_in = code["expires_in"].as_u64().unwrap_or(600);

    let c = ui::Colors::stderr();
    eprintln!("Para iniciar sesión abre {} y confirma el código:", c.bold(&uri));
    eprintln!();
    eprintln!("    {}", c.bold(&c.cyan(&user_code)));
    eprintln!();
    eprintln!("{}", c.dim(&format!("(o abre directamente {complete})")));
    if !no_browser && ui::tty() && open::that_detached(&complete).is_ok() {
        eprintln!("{}", c.dim("Se abrió el navegador. Esperando la aprobación…"));
    } else {
        eprintln!("{}", c.dim("Esperando la aprobación…"));
    }

    let unit = super::poll_unit();
    let deadline = Instant::now() + unit * u32::try_from(expires_in).unwrap_or(600);
    loop {
        tokio::time::sleep(unit * u32::try_from(interval).unwrap_or(5)).await;
        if Instant::now() > deadline {
            return Err(fail_hint(Kind::LoginFailed, "El código caducó sin aprobarse.", "fdt login"));
        }
        let answer = anon
            .raw(Method::POST, "/cli/device/token", &[], Some(&json!({"device_code": device_code})))
            .await?;
        if (200..300).contains(&answer.status) {
            let data = &answer.body["data"];
            let token = data["token"]
                .as_str()
                .ok_or_else(|| fail(Kind::Api, "La API aprobó el login pero no devolvió el token."))?
                .to_owned();
            let who = json!({
                "user": data["user"],
                "token": {"id": data["token_id"], "kind": data["kind"], "expires_at": data["expires_at"]},
            });
            let (store, _) = save_token(ctx, &token, Some(&who)).await?;
            ui::success(&format!(
                "Sesión iniciada como {} (token guardado en el {}).",
                s(&data["user"], "email"),
                store.label()
            ));
            let out = json!({
                "ok": true, "api_url": ctx.api_url()?, "store": store, "user": data["user"],
                "token": {"id": data["token_id"], "kind": data["kind"], "expires_at": data["expires_at"]},
            });
            return ctx.out.print(&out, String::new);
        }
        match answer.body["error"]["scope"].as_str().unwrap_or_default() {
            "cli.authorization_pending" => {}
            // Core sends the new interval in error.meta.interval (RFC 8628 §3.5).
            "cli.slow_down" => {
                interval = answer.body["error"]["meta"]["interval"].as_u64().unwrap_or(interval + 5).max(interval);
            }
            "cli.expired_token" => {
                return Err(fail_hint(Kind::LoginFailed, "El código caducó sin aprobarse.", "fdt login"));
            }
            "cli.access_denied" => {
                return Err(fail_hint(Kind::LoginFailed, "Se rechazó el login en el panel.", "fdt login"));
            }
            _ => return Err(api_error(answer.status, &answer.body).into()),
        }
    }
}

pub async fn logout(ctx: &mut Ctx) -> Result<()> {
    let Some(info) = ctx.token()? else {
        ui::info("No había sesión en esta máquina.");
        return ctx.out.print(&json!({"ok": true, "revoked": false, "removed": false}), String::new);
    };
    let api = Api::new(&ctx.api_url()?, Some(info.token.clone()))?;
    let session = info
        .key
        .as_ref()
        .and_then(|k| ctx.config.credentials.get(k))
        .is_some_and(|c| c.kind.as_deref() == Some("session"));
    let mut cascade = 0;
    let revoked = match api.raw(Method::DELETE, "/cli/session", &[], None).await {
        Ok(a) if (200..300).contains(&a.status) => {
            cascade = a.body["data"]["cascade_revoked"].as_u64().unwrap_or(0);
            // Logout revokes only this token; the ones created with it live
            // on until they expire or are revoked (fdt token list).
            if session {
                ui::info("Los tokens creados con esta sesión siguen vivos: fdt token list / fdt token revoke, o desde el panel.");
            }
            true
        }
        Ok(a) if matches!(a.status, 401 | 404) => false,
        Ok(a) => {
            ui::warn(&format!("No se pudo revocar el token en el servidor: {}", api_error(a.status, &a.body)));
            false
        }
        Err(e) => {
            ui::warn(&format!("No se pudo revocar el token en el servidor: {e}"));
            false
        }
    };
    let removed = match &info.key {
        Some(key) => {
            secrets::delete(&ctx.home, key)?;
            ctx.config.credentials.remove(key);
            ctx.config.save(&ctx.home)?;
            true
        }
        None => {
            ui::warn("El token viene de FDT_TOKEN: quítalo de tu entorno; fdt no lo guarda.");
            false
        }
    };
    ui::success("Sesión cerrada.");
    ctx.out.print(
        &json!({"ok": true, "revoked": revoked, "cascade_revoked": cascade, "removed": removed, "source": info.source}),
        String::new,
    )
}

pub async fn whoami(ctx: &Ctx) -> Result<()> {
    let api = ctx.api()?;
    let mut data = api.get("/cli/whoami").await?;
    let source = ctx.token()?.map(|t| t.source);
    data["token_source"] = json!(source);
    data["active_org"] = json!(ctx.config.org);
    if let Some((path, link)) = &ctx.link {
        data["link"] = json!({"path": path, "org": link.org_name.clone().or(link.org.clone()), "project": link.project_name.clone().or(link.project.clone()), "service": link.service_name.clone().or(link.service.clone())});
    }
    ctx.out.print(&data, || {
        let t = &data["token"];
        let mut rows = vec![
            ("usuario", format!("{} ({})", s(&data["user"], "email"), s(&data["user"], "name"))),
            ("token", if t.is_null() { "JWT del panel".into() } else { format!("{} · {} · nivel {}", s(t, "name"), s(t, "kind"), s(t, "level")) }),
        ];
        if !t.is_null() {
            rows.push(("caduca", when(t, "expires_at")));
            rows.push(("organización del token", s(t, "organization_id")));
        }
        rows.push(("origen", source.unwrap_or("-").to_owned()));
        rows.push(("org activa", ctx.config.org.as_ref().map_or("-".into(), |o| o.name.clone())));
        fields(&rows)
    })
}

fn token_items(data: &Value) -> Vec<Value> {
    data.as_array()
        .or_else(|| data["tokens"].as_array())
        .cloned()
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub async fn token_create(
    ctx: &Ctx,
    name: String,
    level: String,
    projects: Vec<String>,
    expires_in_days: u16,
    reveal: bool,
    output: Option<std::path::PathBuf>,
    dry: bool,
) -> Result<()> {
    // The secret comes back once: decide where it goes before creating it.
    if !dry && !reveal && output.is_none() {
        return Err(Failure::new(
            Kind::MissingInput,
            "El token se muestra una sola vez: indica --output <archivo> (se escribe con permisos 0600) o --reveal para imprimirlo.",
        )
        .hint(format!("fdt token create {name} --output .fdt-token"))
        .into());
    }
    let (api, org) = super::org_api(ctx).await?;
    let mut project_ids = Vec::new();
    for p in &projects {
        project_ids.push(resolve::id(&resolve::project(&api, &org.id, p).await?));
    }
    let body = json!({
        "name": name, "organization_id": org.id, "project_ids": project_ids,
        "level": level, "expires_in_days": expires_in_days,
    });
    if dry {
        return dry_run(ctx, "token create", "POST", "/users/me/cli-tokens", Some(body));
    }
    let mut data = api.post("/users/me/cli-tokens", &body).await?;
    let token = data["token"].as_str().unwrap_or_default().to_owned();
    if let Some(path) = &output {
        secrets::write_private(path, &format!("{token}\n"))?;
        ui::success(&format!("Token guardado en {} (0600). Úsalo como FDT_TOKEN=$(cat {}).", path.display(), path.display()));
        data["token"] = json!(secrets::MASK);
        data["token_file"] = json!(path);
    }
    if reveal {
        ui::warn("Guarda el token ahora: no se vuelve a mostrar. Úsalo como FDT_TOKEN.");
    }
    ctx.out.print(&data, || {
        let mut rows = vec![
            ("id", s(&data, "id")),
            ("nivel", level.clone()),
            ("organización", org.name.clone()),
            ("caduca", when(&data, "expires_at")),
        ];
        if reveal {
            rows.insert(0, ("token", s(&data, "token")));
        }
        fields(&rows)
    })
}

pub async fn token_list(ctx: &Ctx) -> Result<()> {
    let api = ctx.api()?;
    let data = api.get("/users/me/cli-tokens").await?;
    let items = token_items(&data);
    ctx.out.print(&data, || {
        let rows: Vec<Vec<String>> = items
            .iter()
            .map(|t| {
                // Tokens created with a session token die with it (revoking
                // that session revokes them): show which one.
                let by = t["created_by_token_id"].as_str().map_or_else(|| "-".to_owned(), |id| id.chars().take(8).collect());
                vec![s(t, "name"), s(t, "kind"), s(t, "level"), s(t, "organization_id"), when(t, "expires_at"), when(t, "last_used_at"), by, s(t, "id")]
            })
            .collect();
        ui::table(&["nombre", "clase", "nivel", "organización", "caduca", "último uso", "creado con", "id"], &rows)
    })
}

pub async fn token_revoke(ctx: &Ctx, query: String, dry: bool) -> Result<()> {
    let api = ctx.api()?;
    // An id goes straight to the API: a scoped token cannot list tokens but
    // can revoke itself (fdt token revoke <su id>, from fdt whoami).
    let token = match uuid::Uuid::parse_str(query.trim()) {
        Ok(id) => json!({"id": id.to_string(), "name": id.to_string()}),
        Err(_) => {
            let items = token_items(&api.get("/users/me/cli-tokens").await?);
            resolve::pick(&items, &query, &["name"], "token", "fdt token list")?
        }
    };
    let path = format!("/users/me/cli-tokens/{}", enc(&resolve::id(&token)));
    if dry {
        return dry_run(ctx, "token revoke", "DELETE", &path, None);
    }
    let data = api.delete(&path).await?;
    // Revoking a session token also revokes the tokens created with it.
    let cascade = data["cascade_revoked"].as_u64().unwrap_or(0);
    ui::success(&format!("Token «{}» revocado.", s(&token, "name")));
    if cascade > 0 {
        ui::info(&format!("También se revocaron {cascade} token(s) creados con esa sesión."));
    }
    ctx.out.print(&json!({"ok": true, "revoked": resolve::id(&token), "cascade_revoked": cascade}), String::new)
}
