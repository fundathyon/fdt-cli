//! What every command shares: flags, local config, the `.fdt.toml` link,
//! the token and the organization/project/service it works on.

use crate::api::Api;
use crate::config::{self, Config, DEFAULT_SCOPE, Link, OrgRef, credential_key};
use crate::failure::{Failure, Kind, fail, fail_hint};
use crate::output::Out;
use crate::resolve;
use crate::secrets;
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;

pub struct Ctx {
    pub out: Out,
    /// `--json` was passed (not just a missing terminal).
    pub json_flag: bool,
    /// A person can be asked: stdin and stdout are a TTY and no `--no-input`.
    pub interactive: bool,
    /// `--no-input` was passed.
    pub no_input: bool,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub config: Config,
    pub link: Option<(PathBuf, Link)>,
    pub org_flag: Option<String>,
    pub api_url_flag: Option<String>,
}

/// Where the token in use came from.
#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub token: String,
    /// `env`, `keyring` or `file`.
    pub source: &'static str,
    /// Credential key in config.toml, when it was saved.
    pub key: Option<String>,
}

impl Ctx {
    pub fn new(json: bool, no_input: bool, org: Option<String>, api_url: Option<String>) -> Result<Self> {
        let tty = crate::ui::tty();
        let home = config::home()?;
        let cwd = std::env::current_dir()?;
        let config = Config::load(&home)?;
        let link = config::find_link(&cwd)?;
        Ok(Self {
            out: Out { json: json || !std::io::IsTerminal::is_terminal(&std::io::stdout()) },
            json_flag: json,
            interactive: tty && !no_input,
            no_input,
            home,
            cwd,
            config,
            link,
            org_flag: org.filter(|s| !s.trim().is_empty()),
            api_url_flag: api_url,
        })
    }

    /// `--api-url`, `FDT_API_URL`, the one saved by login, or the public gateway.
    pub fn api_url(&self) -> Result<String> {
        match &self.api_url_flag {
            Some(url) => config::normalize_api_url(url),
            None => Ok(self.config.api_url.clone().unwrap_or_else(|| crate::DEFAULT_API_URL.to_owned())),
        }
    }

    pub fn link_value(&self, f: impl Fn(&Link) -> Option<String>) -> Option<String> {
        self.link.as_ref().and_then(|(_, l)| f(l))
    }

    /// Fails asking for a flag when nobody can be asked.
    pub fn need(&self, flag: &str, why: &str) -> anyhow::Error {
        Failure::new(Kind::MissingInput, format!("Falta {flag}: {why}"))
            .hint(format!("vuelve a ejecutarlo con {flag}"))
            .into()
    }

    /// The token: `FDT_TOKEN`, the one saved for the org in context, or the session one.
    pub fn token(&self) -> Result<Option<TokenInfo>> {
        if let Ok(token) = std::env::var("FDT_TOKEN") {
            let token = token.trim().to_owned();
            if !token.is_empty() {
                return Ok(Some(TokenInfo { token, source: "env", key: None }));
            }
        }
        let api = self.api_url()?;
        let mut scopes = Vec::new();
        if let Some(org) = self.local_org_id() {
            scopes.push(org);
        }
        scopes.push(DEFAULT_SCOPE.to_owned());
        for scope in scopes {
            let key = credential_key(&api, &scope);
            if let Some(cred) = self.config.credentials.get(&key)
                && let Some(token) = secrets::load(&self.home, &key, cred.store)?
            {
                let source = match cred.store {
                    secrets::Store::Keyring => "keyring",
                    secrets::Store::File => "file",
                };
                return Ok(Some(TokenInfo { token, source, key: Some(key) }));
            }
        }
        // A single scoped token saved for this API.
        let prefix = format!("{api}#");
        let mine: Vec<_> = self.config.credentials.iter().filter(|(k, _)| k.starts_with(&prefix)).collect();
        if let [(key, cred)] = mine.as_slice()
            && let Some(token) = secrets::load(&self.home, key, cred.store)?
        {
            let source = if cred.store == secrets::Store::Keyring { "keyring" } else { "file" };
            return Ok(Some(TokenInfo { token, source, key: Some((*key).clone()) }));
        }
        Ok(None)
    }

    /// An org id known without asking the API: `--org` if it is a saved id,
    /// the link, or the active org.
    fn local_org_id(&self) -> Option<String> {
        if let Some(flag) = &self.org_flag {
            let api = self.api_url().ok()?;
            if self.config.credentials.contains_key(&credential_key(&api, flag)) {
                return Some(flag.clone());
            }
            return None;
        }
        self.link_value(|l| l.org.clone())
            .or_else(|| self.config.org.as_ref().map(|o| o.id.clone()))
    }

    /// A client without a token (for the public device-login routes).
    pub fn anonymous(&self) -> Result<Api> {
        Api::new(&self.api_url()?, None)
    }

    /// A client with the token; exit code 3 if there is none.
    pub fn api(&self) -> Result<Api> {
        match self.token()? {
            Some(info) => Api::new(&self.api_url()?, Some(info.token)),
            None => Err(fail_hint(
                Kind::Unauthenticated,
                "No hay sesión de fdt.",
                "fdt login (o exporta FDT_TOKEN con un token fdt_…)",
            )),
        }
    }

    /// The organization to work on: `--org`, `.fdt.toml`, `fdt use`, or the only one.
    pub async fn org(&self, api: &Api) -> Result<OrgRef> {
        if let Some(query) = &self.org_flag {
            let item = resolve::org(api, query).await?;
            return Ok(org_ref(&item));
        }
        if let Some(id) = self.link_value(|l| l.org.clone()) {
            let name = self.link_value(|l| l.org_name.clone()).unwrap_or_else(|| id.clone());
            return Ok(OrgRef { id, name });
        }
        if let Some(org) = &self.config.org {
            return Ok(org.clone());
        }
        let orgs = resolve::orgs(api).await?;
        match orgs.len() {
            0 => Err(fail(Kind::NotFound, "Tu usuario no pertenece a ninguna organización.")),
            1 => Ok(org_ref(&orgs[0])),
            _ if self.interactive => {
                let labels: Vec<String> = orgs.iter().map(|o| crate::output::s(o, "name")).collect();
                let i = crate::ui::select("Organización", &labels)?;
                Ok(org_ref(&orgs[i]))
            }
            _ => Err(Failure::new(Kind::MissingInput, "Perteneces a varias organizaciones: indica cuál.")
                .hint("fdt use <org>, o --org <org> en cada comando")
                .details(serde_json::json!({
                    "candidates": orgs.iter().map(|o| crate::output::s(o, "name")).collect::<Vec<_>>()
                }))
                .into()),
        }
    }

    /// The project in `query`, or the linked one; `None` if neither.
    pub async fn project_opt(&self, api: &Api, org: &OrgRef, query: Option<&str>) -> Result<Option<Value>> {
        if let Some(q) = query {
            return resolve::project(api, &org.id, q).await.map(Some);
        }
        if self.link_value(|l| l.org.clone()).as_deref() == Some(org.id.as_str())
            && let Some(id) = self.link_value(|l| l.project.clone())
        {
            let name = self.link_value(|l| l.project_name.clone()).unwrap_or_else(|| id.clone());
            return Ok(Some(serde_json::json!({"id": id, "name": name})));
        }
        Ok(None)
    }

    /// The project in `query` or the linked one; fails if neither.
    pub async fn project(&self, api: &Api, org: &OrgRef, query: Option<&str>) -> Result<Value> {
        match self.project_opt(api, org, query).await? {
            Some(p) => Ok(p),
            None if self.interactive => {
                let projects = resolve::projects(api, &org.id).await?;
                if projects.is_empty() {
                    return Err(fail_hint(Kind::NotFound, "La organización no tiene proyectos.", "fdt project create <nombre>"));
                }
                let labels: Vec<String> = projects.iter().map(|p| crate::output::s(p, "name")).collect();
                let i = crate::ui::select("Proyecto", &labels)?;
                Ok(projects[i].clone())
            }
            None => Err(self.need("--project <proyecto>", "no hay proyecto en .fdt.toml (fdt link)")),
        }
    }

    /// The service in `query` or the linked one.
    pub async fn service(&self, api: &Api, org: &OrgRef, query: Option<&str>) -> Result<Value> {
        let query = match query {
            Some(q) => q.to_owned(),
            None => match self.link_value(|l| l.service.clone()) {
                Some(id) if self.link_value(|l| l.org.clone()).as_deref() == Some(org.id.as_str()) => id,
                _ => {
                    if self.interactive {
                        let services = resolve::services(api, &org.id).await?;
                        if services.is_empty() {
                            return Err(fail_hint(Kind::NotFound, "La organización no tiene servicios.", "fdt service create"));
                        }
                        let labels: Vec<String> = services.iter().map(|s| crate::output::s(s, "slug")).collect();
                        let i = crate::ui::select("Servicio", &labels)?;
                        return Ok(services[i].clone());
                    }
                    return Err(self.need("el servicio (argumento o --service)", "no hay servicio en .fdt.toml (fdt link)"));
                }
            },
        };
        resolve::service(api, &org.id, &query).await
    }

    /// Destructive commands: `--confirm <exact name>`, or typed at a terminal.
    /// `action` is what happens, e.g. "borrar el proyecto".
    pub fn confirm(&self, expected: &str, given: Option<&str>, action: &str) -> Result<()> {
        match given {
            Some(g) if g == expected => Ok(()),
            Some(g) => Err(Failure::new(
                Kind::ConfirmationRequired,
                format!("--confirm «{g}» no coincide con el nombre exacto «{expected}»."),
            )
            .hint(format!("--confirm {expected}"))
            .into()),
            None if self.interactive => {
                let typed = crate::ui::ask(&format!("Vas a {action} «{expected}». Escribe su nombre para confirmar"))?;
                if typed == expected {
                    Ok(())
                } else {
                    Err(fail(Kind::Cancelled, "El nombre no coincide; no se hizo nada."))
                }
            }
            None => Err(Failure::new(
                Kind::ConfirmationRequired,
                format!("{} «{expected}» exige --confirm con su nombre exacto.", capitalize(action)),
            )
            .hint(format!("--confirm {expected}"))
            .into()),
        }
    }
}

pub fn org_ref(item: &Value) -> OrgRef {
    OrgRef {
        id: resolve::id(item),
        name: item["name"].as_str().unwrap_or_default().to_owned(),
    }
}

/// What a `--dry-run` prints: the request that would be sent, without secret values.
pub fn dry_run(ctx: &Ctx, action: &str, method: &str, path: &str, body: Option<Value>) -> Result<()> {
    dry_run_with(ctx, action, method, path, body, Value::Null)
}

/// `dry_run` plus extra fields (what the server would do, when fdt can tell).
pub fn dry_run_with(ctx: &Ctx, action: &str, method: &str, path: &str, body: Option<Value>, extra: Value) -> Result<()> {
    let mut data = serde_json::json!({
        "dry_run": true, "action": action, "method": method, "path": path, "body": body,
    });
    if let Value::Object(map) = extra {
        for (k, v) in map {
            data[k] = v;
        }
    }
    crate::ui::info(&format!("--dry-run: no se cambió nada ({action})."));
    ctx.out.print(&data, || {
        let mut text = format!("{method} {path}");
        if let Some(body) = &data["body"].as_object() {
            text.push('\n');
            text.push_str(&serde_json::to_string_pretty(body).unwrap_or_default());
        }
        text
    })
}

fn capitalize(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
