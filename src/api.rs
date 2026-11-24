//! HTTP client for Core behind the gateway. Every route is relative to the
//! base URL (which already ends in `/api/v1`). Core answers with an envelope
//! `{success, status_code, data, meta, error: {code, message, scope, meta}}`;
//! errors become a [`Failure`] with the exit code the scope or status asks for.

use crate::failure::{Failure, Kind, fail};
use anyhow::Result;
use reqwest::Method;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct Api {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

/// A raw answer: status and the parsed body (`Value::Null` if empty, a string if not JSON).
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
    /// Seconds of the `Retry-After` header (a 429), when it is a number.
    pub retry_after: Option<u64>,
}

impl Answer {
    /// A rate limit (429 or `cli.rate_limited`): how long to wait, from
    /// `Retry-After` or `error.meta.retry_after`; `None` if it is not one.
    pub fn rate_limited(&self) -> Option<Option<u64>> {
        let limited = self.status == 429 || self.body["error"]["scope"] == "cli.rate_limited";
        limited.then(|| self.retry_after.or_else(|| self.body["error"]["meta"]["retry_after"].as_u64()))
    }
}

/// Percent-encodes one path segment.
pub fn enc(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn user_agent() -> String {
    format!("fdt/{} ({}; {})", crate::VERSION, std::env::consts::OS, std::env::consts::ARCH)
}

impl Api {
    pub fn new(base: &str, token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            .connect_timeout(Duration::from_secs(10))
            // The gateway cuts every request at 30 s.
            .timeout(Duration::from_secs(40))
            .build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            token,
            http,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{path}", self.base)
        } else {
            format!("{}/{path}", self.base)
        }
    }

    /// Sends a request and returns whatever came back, without judging the status.
    pub async fn raw(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<Answer> {
        let mut request = self
            .http
            .request(method, self.url(path))
            .header("accept", "application/json");
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|e| {
            let what = if e.is_timeout() { "tardó demasiado" } else { "no respondió" };
            fail(
                Kind::Network,
                crate::secrets::redact(&format!(
                    "La API de Foundathyon ({}) {what}: {}",
                    self.base,
                    e.without_url()
                )),
            )
        })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let text = response
            .text()
            .await
            .map_err(|e| fail(Kind::Network, format!("No se pudo leer la respuesta: {e}")))?;
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok(Answer { status, body, retry_after })
    }

    /// The whole envelope of a successful answer, or the mapped error.
    pub async fn envelope(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let answer = self.raw(method, path, query, body).await?;
        if (200..300).contains(&answer.status) && answer.body["success"] != json!(false) {
            return Ok(answer.body);
        }
        Err(api_error(answer.status, &answer.body).into())
    }

    /// `data` of a successful answer.
    pub async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        Ok(self.envelope(method, path, &[], body).await?["data"].take())
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn get_query(&self, path: &str, query: &[(String, String)]) -> Result<Value> {
        Ok(self.envelope(Method::GET, path, query, None).await?["data"].take())
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::PUT, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }
}

/// The command that usually fixes an API error scope.
pub fn hint_for_scope(scope: &str) -> Option<&'static str> {
    Some(match scope {
        s if s.starts_with("quota.") => "fdt org usage (y sube de plan en el panel)",
        "deploy.workflow_missing" => {
            "fdt service install-workflow <servicio> --mode pull_request (y fusiona el pull request), o --mode commit"
        }
        "deploy.permission_missing" => {
            "acepta los permisos nuevos de la GitHub App de Foundathyon en GitHub y vuelve a intentarlo"
        }
        "deploy.no_image" => "fdt deploy <servicio> (lanza el build del workflow)",
        "deploy.slug_taken" => "elige otro subdominio, o usa --if-not-exists si el servicio ya es de esta app",
        "deploy.slug_reserved" | "deploy.subdomain_invalid" => {
            "elige otro subdominio: [a-z0-9-], sin guion en los extremos; www, api, admin, app, db… son de Foundathyon"
        }
        "deploy.image_name_taken" => "otro servicio de la organización ya usa la imagen de ese repo: fdt service list",
        "deploy.database_name_taken" => "elige otro nombre o usa --if-not-exists",
        "deploy.database_not_ready" => "fdt db show <base> (espera a que esté ready)",
        "deploy.app_not_found" => "fdt api GET /organizations/{org}/apps (revisa la app o el repo)",
        "deploy.registry_slug_missing" | "deploy.registry_not_configured" => {
            "configura el registry de la organización en el panel"
        }
        "cli.expired_token" | "cli.access_denied" | "cli.invalid_token" => "fdt login (o exporta FDT_TOKEN con un token válido)",
        "cli.token_level" => {
            "el nivel del token no alcanza: read solo lee (sin secretos); deploy añade deploy, redeploy, variables, \
             PATCH del servicio y las credenciales de bases (db credentials --reveal, db url, env set --from-db); \
             admin, todo lo demás (crear, borrar, rollback, restart, pause, resume, workflow) y las lecturas con secretos \
             de la organización (invitaciones, tokens del registry, secretos de CI, credenciales de apps). \
             Crea otro con fdt token create <nombre> --level admin (o --level deploy si basta), o usa tu sesión: fdt login"
        }
        "cli.token_scope" => {
            "el token no cubre esta organización o proyecto (fdt whoami muestra su alcance). \
             Un token limitado a proyectos no crea proyectos, servicios ni bases ni ve miembros: \
             usa un token de la organización sin --project (fdt token create <nombre> --level admin) o tu sesión (fdt login)"
        }
        "cli.token_forbidden" => {
            "esto no se puede hacer con ningún token fdt_ (o un token scoped no gestiona tokens): \
             hazlo en el panel, o con tu sesión de fdt login si es gestión de tokens"
        }
        "cli.invalid_project" => "cada --project tiene que ser un proyecto de la organización (fdt project list)",
        "cli.too_many_tokens" => "revoca los que ya no uses: fdt token list y fdt token revoke <nombre>",
        "cli.not_configured" => "Core no tiene configurado el login del CLI (ADMIN_URL): avisa a quien opera la plataforma; mientras, usa fdt login --with-token",
        "cli.rate_limited" => "demasiadas peticiones: espera error.details.meta.retry_after segundos y reintenta",
        _ => return None,
    })
}

/// Core answers a GET that returns a secret, to a token below the level it
/// needs, with "… needs a token of level <level> or higher": say that level.
fn sensitive_read_hint(scope: Option<&str>, message: &str) -> Option<String> {
    if scope != Some("cli.token_level") {
        return None;
    }
    let level = message.split("needs a token of level ").nth(1)?.split_whitespace().next()?;
    matches!(level, "deploy" | "admin").then(|| {
        format!(
            "esta lectura devuelve un secreto y pide un token de nivel {level} o más: \
             fdt token create <nombre> --level {level} (o usa tu sesión: fdt login)"
        )
    })
}

/// Whether an answer is Core's envelope (`{success, error: {…}}`) and not
/// something the gateway or a proxy wrote (`404 page not found`).
pub fn from_core(body: &Value) -> bool {
    body.get("success").is_some() || body["error"].is_object()
}

/// A Failure from an API error answer.
pub fn api_error(status: u16, body: &Value) -> Failure {
    let error = &body["error"];
    let scope = error["scope"].as_str().map(str::to_owned);
    let mut message = error["message"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| body.as_str().map(|s| s.trim().chars().take(300).collect()))
        .unwrap_or_else(|| format!("La API respondió {status}"));
    let gateway_404 = status == 404 && !from_core(body);
    if gateway_404 {
        message = format!(
            "La API respondió 404 sin la respuesta de Core («{message}»): la ruta no existe o el gateway no la expone"
        );
    }
    let quota = scope.as_deref().is_some_and(|s| s.starts_with("quota."));
    let kind = match status {
        401 => Kind::Unauthenticated,
        403 if quota => Kind::Quota,
        403 => Kind::Forbidden,
        404 => Kind::NotFound,
        _ if quota => Kind::Quota,
        _ => Kind::Api,
    };
    let hint = sensitive_read_hint(scope.as_deref(), &message)
        .or_else(|| scope.as_deref().and_then(hint_for_scope).map(str::to_owned))
        .or_else(|| match kind {
            Kind::Unauthenticated => Some("fdt login (o exporta FDT_TOKEN con un token fdt_…)".to_owned()),
            Kind::Forbidden => Some(
                "fdt whoami (revisa el nivel y el alcance del token, o tu rol en la organización)".to_owned(),
            ),
            Kind::NotFound if gateway_404 => Some(
                "revisa la ruta: es relativa a la URL de la API (…/api/v1), sin repetir /api/v1 (fdt context la muestra)".to_owned(),
            ),
            _ => None,
        });
    let mut details = serde_json::Map::new();
    if !error["meta"].is_null() {
        details.insert("meta".into(), error["meta"].clone());
    }
    if let Some(trace) = body["trace_id"].as_str() {
        details.insert("trace_id".into(), json!(trace));
    }
    Failure {
        kind,
        message: crate::secrets::redact(&message),
        scope,
        status: Some(status),
        hint,
        details: (!details.is_empty()).then_some(Value::Object(details)),
    }
}

/// Rewrites a `cli.token_scope` 403 so it says why: a token limited to
/// projects (`project_ids`) cannot do org-wide things like creating
/// projects, services or databases, or listing members.
pub fn explain_scope(error: anyhow::Error, what: &str) -> anyhow::Error {
    match crate::failure::failure_of(&error) {
        Some(f) if f.scope.as_deref() == Some("cli.token_scope") => {
            let mut f = f.clone();
            f.message = format!(
                "No se puede {what} con este token: está limitado a otra organización o a unos proyectos \
                 (project_ids), y Core solo deja a esos tokens trabajar dentro de sus proyectos. Core: «{}»",
                f.message
            );
            f.into()
        }
        _ => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(status: u16, scope: &str) -> Failure {
        api_error(status, &json!({"success": false, "error": {"code": status, "message": "m", "scope": scope}}))
    }

    #[test]
    fn maps_status_and_scope_to_kinds() {
        assert_eq!(err(401, "auth.invalid").kind, Kind::Unauthenticated);
        assert_eq!(err(403, "deploy.forbidden").kind, Kind::Forbidden);
        assert_eq!(err(403, "quota.deploy.services").kind, Kind::Quota);
        assert_eq!(err(404, "deploy.service_not_found").kind, Kind::NotFound);
        assert_eq!(err(409, "deploy.slug_taken").kind, Kind::Api);
        assert_eq!(err(403, "quota.deploy.services").hint.unwrap(), "fdt org usage (y sube de plan en el panel)");
        assert_eq!(err(401, "x").hint.unwrap(), "fdt login (o exporta FDT_TOKEN con un token fdt_…)");
    }

    #[test]
    fn sensitive_reads_name_the_level_they_need() {
        let body = json!({"success": false, "error": {"code": 403, "scope": "cli.token_level",
            "message": "This read returns a secret and needs a token of level deploy or higher"}});
        let hint = api_error(403, &body).hint.unwrap();
        assert!(hint.contains("--level deploy"), "{hint}");
        let generic = err(403, "cli.token_level").hint.unwrap();
        assert!(generic.contains("--level admin") && generic.contains("credenciales de bases"), "{generic}");
        assert!(err(422, "cli.invalid_project").hint.unwrap().contains("fdt project list"));
    }

    #[test]
    fn a_404_that_is_not_from_core_says_the_gateway_may_not_expose_the_route() {
        let f = api_error(404, &json!("404 page not found"));
        assert_eq!(f.kind, Kind::NotFound);
        assert!(f.scope.is_none());
        assert!(f.message.contains("gateway") && f.message.contains("404 page not found"), "{}", f.message);
        let core = api_error(404, &json!({"success": false, "error": {"message": "Service not found", "scope": "deploy.service_not_found"}}));
        assert_eq!(core.message, "Service not found");
    }

    #[test]
    fn encodes_segments() {
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
        assert_eq!(enc("ABCD-EFGH"), "ABCD-EFGH");
    }
}
