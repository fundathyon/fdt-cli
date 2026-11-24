//! End-to-end tests: the real `fdt` binary against a mock Core (wiremock).
//! No terminal, so output is JSON and nothing prompts.

use serde_json::{Value, json};
use std::path::Path;
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "fdt_tok1_S3cr3tValueThatMustNotLeak";
const ORG: &str = "11111111-1111-1111-1111-111111111111";
const SVC: &str = "22222222-2222-2222-2222-222222222222";
const PROJECT: &str = "33333333-3333-3333-3333-333333333333";
const DB: &str = "44444444-4444-4444-4444-444444444444";

struct Env {
    home: TempDir,
    cwd: TempDir,
    server: MockServer,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}\nstderr: {}", self.stdout, self.stderr))
    }

    /// The JSON error: the last block of stderr (progress messages may come before it).
    fn error(&self) -> Value {
        let start = self.stderr.rfind("\n{").map_or(0, |i| i + 1);
        serde_json::from_str::<Value>(&self.stderr[start..]).unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {}", self.stderr))["error"].clone()
    }

    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": true, "status_code": 200, "data": data}))
}

fn err(status: u16, scope: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "success": false, "status_code": status,
        "error": {"code": status, "message": format!("falló {scope}"), "scope": scope}
    }))
}

impl Env {
    async fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
            cwd: TempDir::new().unwrap(),
            server: MockServer::start().await,
        }
    }

    /// Mocks the user's single organization.
    async fn with_org(self) -> Self {
        Mock::given(method("GET"))
            .and(path("/users/me/has-organization"))
            .respond_with(ok(json!({"has_organization": true, "user_registered": true,
                "organizations": [{"id": ORG, "name": "Acme", "roles": ["owner"]}]})))
            .mount(&self.server)
            .await;
        self
    }

    async fn with_service(self) -> Self {
        Mock::given(method("GET"))
            .and(path(format!("/organizations/{ORG}/deploy/services")))
            .respond_with(ok(json!([{"id": SVC, "slug": "api", "name": "API", "status": "live",
                "url": "https://api-acme.foundathyon.dev", "app_id": "app1", "latest_deployment": null}])))
            .mount(&self.server)
            .await;
        self
    }

    async fn run(&self, args: &[&str]) -> Run {
        self.run_with(args, &[("FDT_TOKEN", TOKEN)], None).await
    }

    async fn run_with(&self, args: &[&str], vars: &[(&str, &str)], stdin: Option<&str>) -> Run {
        self.run_in(self.cwd.path(), args, vars, stdin).await
    }

    async fn run_in(&self, dir: &Path, args: &[&str], vars: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use tokio::io::AsyncWriteExt;
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_fdt"));
        cmd.args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
            .env("FDT_HOME", self.home.path().join(".fdt"))
            .env("FDT_API_URL", self.server.uri())
            .env("FDT_NO_KEYRING", "1")
            .env("FDT_POLL_UNIT_MS", "5")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).await.unwrap();
        }
        drop(input);
        let out = child.wait_with_output().await.unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

async fn mount_device_code(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/cli/device/code"))
        .respond_with(ok(json!({
            "device_code": "dev-123", "user_code": "ABCD-EFGH",
            "verification_uri": "https://app.foundathyon.com/cli",
            "verification_uri_complete": "https://app.foundathyon.com/cli?code=ABCD-EFGH",
            "expires_in": 600, "interval": 1,
        })))
        .expect(1)
        .mount(server)
        .await;
}

// ---------------------------------------------------------------- login

#[tokio::test]
async fn device_login_waits_through_pending_and_slow_down_then_saves_the_token() {
    let env = Env::new().await;
    mount_device_code(&env.server).await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .and(body_json(json!({"device_code": "dev-123"})))
        .respond_with(err(400, "cli.authorization_pending"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .respond_with(err(400, "cli.slow_down"))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .respond_with(ok(json!({
            "token": TOKEN, "token_id": "tok1", "kind": "session", "expires_at": "2027-02-21T00:00:00Z",
            "user": {"id": "u1", "email": "ana@acme.dev", "name": "Ana"},
        })))
        .with_priority(3)
        .mount(&env.server)
        .await;

    let run = env.run_with(&["login"], &[], None).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let out = run.json();
    assert_eq!(out["user"]["email"], "ana@acme.dev");
    assert_eq!(out["token"]["kind"], "session");
    assert_eq!(out["store"], "file");
    assert!(run.stderr.contains("ABCD-EFGH"), "shows the user code: {}", run.stderr);
    assert!(!run.all().contains("S3cr3t"), "the token never reaches the output");
    let requests = env.server.received_requests().await.unwrap();
    assert_eq!(requests.iter().filter(|r| r.url.path() == "/cli/device/token").count(), 3);
    let code_request = requests.iter().find(|r| r.url.path() == "/cli/device/code").unwrap();
    let body: Value = serde_json::from_slice(&code_request.body).unwrap();
    assert!(body["client_name"].as_str().unwrap().starts_with("fdt "));
    assert!(code_request.headers.get("user-agent").unwrap().to_str().unwrap().starts_with("fdt/"));

    // Saved with mode 0600 and used by the next command.
    let creds = env.home.path().join(".fdt/credentials.toml");
    assert!(std::fs::read_to_string(&creds).unwrap().contains(TOKEN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&creds).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let config = std::fs::read_to_string(env.home.path().join(".fdt/config.toml")).unwrap();
    assert!(!config.contains("S3cr3t"), "config.toml never holds the token");
    Mock::given(method("GET"))
        .and(path("/cli/whoami"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ok(json!({"user": {"id": "u1", "email": "ana@acme.dev", "name": "Ana"},
            "token": {"id": "tok1", "name": "fdt login", "kind": "session", "level": null}})))
        .mount(&env.server)
        .await;
    let who = env.run_with(&["whoami"], &[], None).await;
    assert_eq!(who.code, 0, "{}", who.stderr);
    assert_eq!(who.json()["user"]["email"], "ana@acme.dev");
    assert_eq!(who.json()["token_source"], "file");
}

#[tokio::test]
async fn device_login_denied_exits_3() {
    let env = Env::new().await;
    mount_device_code(&env.server).await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .respond_with(err(400, "cli.access_denied"))
        .mount(&env.server)
        .await;
    let run = env.run_with(&["login"], &[], None).await;
    assert_eq!(run.code, 3, "{}", run.stderr);
    assert_eq!(run.error()["kind"], "login_failed");
    assert!(!env.home.path().join(".fdt/credentials.toml").exists());
}

#[tokio::test]
async fn device_login_expired_exits_3() {
    let env = Env::new().await;
    mount_device_code(&env.server).await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .respond_with(err(400, "cli.expired_token"))
        .mount(&env.server)
        .await;
    let run = env.run_with(&["login"], &[], None).await;
    assert_eq!(run.code, 3);
    assert_eq!(run.error()["hint"], "fdt login");
}

#[tokio::test]
async fn device_login_refuses_no_input() {
    let env = Env::new().await;
    let run = env.run_with(&["login", "--no-input"], &[], None).await;
    assert_eq!(run.code, 2);
    assert_eq!(run.error()["kind"], "missing_input");
}

#[tokio::test]
async fn login_with_token_from_stdin_checks_it_first() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/cli/whoami"))
        .respond_with(err(401, "cli.invalid_token"))
        .mount(&env.server)
        .await;
    let run = env.run_with(&["login", "--with-token"], &[], Some("fdt_bad_tokenvalue\n")).await;
    assert_eq!(run.code, 3);
    assert!(!env.home.path().join(".fdt/credentials.toml").exists(), "a rejected token is not saved");
    assert!(!run.all().contains("tokenvalue"));
}

#[tokio::test]
async fn scoped_token_login_becomes_the_org_credential() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path("/cli/whoami"))
        .respond_with(ok(json!({"user": {"id": "u1", "email": "ci@acme.dev", "name": "CI"},
            "token": {"id": "tok9", "name": "ci", "kind": "scoped", "level": "deploy", "organization_id": ORG}})))
        .mount(&env.server)
        .await;
    let run = env.run_with(&["login", "--with-token"], &[], Some(TOKEN)).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["scope"], ORG);
    let config = std::fs::read_to_string(env.home.path().join(".fdt/config.toml")).unwrap();
    assert!(config.contains("Acme"), "the token's org becomes the active one: {config}");
}

// ---------------------------------------------------------------- exit codes

#[tokio::test]
async fn without_a_session_commands_exit_3_with_a_hint() {
    let env = Env::new().await;
    let run = env.run_with(&["project", "list"], &[], None).await;
    assert_eq!(run.code, 3);
    let e = run.error();
    assert_eq!(e["kind"], "unauthenticated");
    assert!(e["hint"].as_str().unwrap().contains("fdt login"));
}

#[tokio::test]
async fn api_errors_map_to_exit_codes_and_keep_the_scope() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/usage")))
        .respond_with(err(403, "organizations.access.permission_required"))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/members")))
        .respond_with(err(404, "access.organization.not_found"))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}")))
        .respond_with(err(401, "auth.invalid_token"))
        .mount(&env.server)
        .await;

    let forbidden = env.run(&["org", "usage"]).await;
    assert_eq!(forbidden.code, 4);
    assert_eq!(forbidden.error()["scope"], "organizations.access.permission_required");
    let missing = env.run(&["org", "members"]).await;
    assert_eq!(missing.code, 5);
    let unauth = env.run(&["org", "show"]).await;
    assert_eq!(unauth.code, 3);
    assert!(!unauth.all().contains("S3cr3t"), "the token never appears in errors");
}

#[tokio::test]
async fn plan_quota_exits_6_with_the_fix() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services")))
        .respond_with(ok(json!([])))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/apps")))
        .respond_with(ok(json!([{"id": "app1", "name": "tienda-api", "slug": "tienda-api",
            "repository": {"full_name": "acme/tienda-api"}}])))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/organizations/{ORG}/deploy/services")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"success": false, "status_code": 403,
            "error": {"code": 403, "message": "Deploy service limit reached", "scope": "quota.deploy.services",
                "meta": {"limit_key": "max_deploy_services", "limit": 1, "used": 1}}})))
        .expect(1)
        .mount(&env.server)
        .await;
    let run = env.run(&["service", "create", "api", "--repo", "acme/tienda-api", "--size", "512"]).await;
    assert_eq!(run.code, 6, "{}", run.stderr);
    let e = run.error();
    assert_eq!(e["kind"], "quota");
    assert_eq!(e["scope"], "quota.deploy.services");
    assert!(e["hint"].as_str().unwrap().contains("fdt org usage"));
    assert_eq!(e["details"]["meta"]["limit"], 1);
    let req = env.server.received_requests().await.unwrap().into_iter().find(|r| r.method.as_str() == "POST").unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["app_id"], "app1");
    assert_eq!(body["memory_mb"], 512);
    assert_eq!(body["slug"], "api");
}

#[tokio::test]
async fn usage_errors_exit_2_as_json() {
    let env = Env::new().await;
    let run = env.run(&["--json", "deploy", "--timeout", "nope"]).await;
    assert_eq!(run.code, 2);
    assert_eq!(run.error()["kind"], "usage");
}

// ---------------------------------------------------------------- resolution

#[tokio::test]
async fn names_slugs_and_ids_resolve_and_ambiguity_lists_candidates() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services")))
        .respond_with(ok(json!([
            {"id": SVC, "slug": "api", "name": "Tienda API"},
            {"id": "99999999-9999", "slug": "api-staging", "name": "Tienda API"},
        ])))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services/{SVC}")))
        .respond_with(ok(json!({"id": SVC, "slug": "api", "name": "Tienda API"})))
        .mount(&env.server)
        .await;
    for query in ["api", SVC, "API"] {
        let run = env.run(&["service", "show", query]).await;
        assert_eq!(run.code, 0, "{query}: {}", run.stderr);
        assert_eq!(run.json()["id"], SVC);
    }
    let ambiguous = env.run(&["service", "show", "tienda api"]).await;
    assert_eq!(ambiguous.code, 2);
    let e = ambiguous.error();
    assert_eq!(e["kind"], "ambiguous");
    assert_eq!(e["details"]["candidates"].as_array().unwrap().len(), 2);
    let missing = env.run(&["service", "show", "nope"]).await;
    assert_eq!(missing.code, 5);
    assert_eq!(missing.error()["hint"], "fdt service list");
}

#[tokio::test]
async fn several_orgs_without_a_choice_fail_without_asking() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/users/me/has-organization"))
        .respond_with(ok(json!({"organizations": [{"id": "a", "name": "Uno"}, {"id": "b", "name": "Dos"}]})))
        .mount(&env.server)
        .await;
    let run = env.run(&["project", "list"]).await;
    assert_eq!(run.code, 2);
    assert_eq!(run.error()["kind"], "missing_input");
    let run = env.run(&["use"]).await;
    assert_eq!(run.code, 2);

    // `fdt use` then remembers it.
    let run = env.run(&["use", "dos"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    Mock::given(method("GET"))
        .and(path("/organizations/b/projects"))
        .respond_with(ok(json!([{"id": PROJECT, "name": "Tienda", "services": []}])))
        .mount(&env.server)
        .await;
    let run = env.run(&["project", "list", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()[0]["name"], "Tienda");
}

#[tokio::test]
async fn link_writes_fdt_toml_and_commands_use_it() {
    let env = Env::new().await.with_org().await.with_service().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/projects")))
        .and(query_param("size", "100"))
        .respond_with(ok(json!([{"id": PROJECT, "name": "Tienda"}])))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}")))
        .respond_with(ok(json!({"id": ORG, "name": "Acme"})))
        .mount(&env.server)
        .await;
    let run = env.run(&["link", "--project", "tienda", "--service", "api"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let raw = std::fs::read_to_string(env.cwd.path().join(".fdt.toml")).unwrap();
    assert!(raw.contains(PROJECT) && raw.contains(SVC) && raw.contains(ORG), "{raw}");

    let sub = env.cwd.path().join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    let ctx = env.run_in(&sub, &["context"], &[("FDT_TOKEN", TOKEN)], None).await;
    assert_eq!(ctx.code, 0);
    let c = ctx.json();
    assert_eq!(c["project"], "Tienda");
    assert_eq!(c["service"], "api");
    assert_eq!(c["org_source"], ".fdt.toml");
    assert_eq!(c["token"]["source"], "env");

    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services/{SVC}/logs")))
        .and(query_param("tail", "200"))
        .respond_with(ok(json!({"lines": [
            {"at": "2000-01-01T00:00:00Z", "stream": "stdout", "text": "viejo"},
            {"at": chrono_now(), "stream": "stdout", "text": "nuevo"},
        ]})))
        .mount(&env.server)
        .await;
    let logs = env.run_in(&sub, &["logs", "--since", "10m"], &[("FDT_TOKEN", TOKEN)], None).await;
    assert_eq!(logs.code, 0, "{}", logs.stderr);
    let lines = logs.json()["lines"].as_array().unwrap().clone();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["text"], "nuevo");

    let un = env.run(&["unlink"]).await;
    assert_eq!(un.code, 0);
    assert!(!env.cwd.path().join(".fdt.toml").exists());
}

fn chrono_now() -> String {
    // RFC 3339 "now" without pulling chrono into the test: the system clock in UTC.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, m, d) = civil(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

// ---------------------------------------------------------------- env

#[tokio::test]
async fn env_set_reads_merges_and_writes_the_whole_set_keeping_secrets() {
    let env = Env::new().await.with_org().await.with_service().await;
    let env_path = format!("/organizations/{ORG}/deploy/services/{SVC}/env");
    Mock::given(method("GET"))
        .and(path(env_path.as_str()))
        .respond_with(ok(json!([
            {"key": "MODE", "value": "dev", "is_secret": false},
            {"key": "STRIPE_KEY", "is_secret": true},
        ])))
        .mount(&env.server)
        .await;
    Mock::given(method("PUT"))
        .and(path(env_path.as_str()))
        .and(body_json(json!({"vars": [
            {"key": "MODE", "value": "prod", "is_secret": false},
            {"key": "NEW", "value": "1", "is_secret": false},
            {"key": "STRIPE_KEY", "is_secret": true},
        ]})))
        .respond_with(ok(json!({"vars": [
            {"key": "MODE", "value": "prod", "is_secret": false},
            {"key": "NEW", "value": "1", "is_secret": false},
            {"key": "STRIPE_KEY", "is_secret": true},
        ], "deployment": {"id": "dep-env", "status": "queued"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    let run = env.run(&["env", "set", "MODE=prod", "NEW=1", "--service", "api"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let out = run.json();
    assert_eq!(out["diff"]["added"], json!(["NEW"]));
    assert_eq!(out["diff"]["changed"], json!(["MODE"]));
    assert_eq!(out["deployment"]["id"], "dep-env");

    // Nothing to change → no PUT (the mock above expects exactly one).
    let again = env.run(&["env", "set", "MODE=dev", "--service", "api", "--dry-run"]).await;
    assert_eq!(again.code, 0);
    assert_eq!(again.json()["dry_run"], true);
    let unset = env.run(&["env", "unset", "MISSING", "--service", "api"]).await;
    assert_eq!(unset.code, 0, "{}", unset.stderr);
    assert_eq!(unset.json()["changed"], false);
}

#[tokio::test]
async fn env_set_from_db_never_prints_the_url() {
    let env = Env::new().await.with_org().await.with_service().await;
    let env_path = format!("/organizations/{ORG}/deploy/services/{SVC}/env");
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/databases")))
        .respond_with(ok(json!({"databases": [{"id": DB, "name": "tienda-pg", "engine": "postgres", "status": "ready"}]})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/databases/{DB}/credentials")))
        .respond_with(ok(json!({"username": "u", "password": "Pa55word!",
            "internal_connection_string": "postgres://u:Pa55word!@db-1:5432/app?sslmode=disable"})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(env_path.as_str()))
        .respond_with(ok(json!([])))
        .mount(&env.server)
        .await;
    Mock::given(method("PUT"))
        .and(path(env_path.as_str()))
        .and(body_json(json!({"vars": [{"key": "DATABASE_URL",
            "value": "postgres://u:Pa55word!@db-1:5432/app?sslmode=disable", "is_secret": true}]})))
        .respond_with(ok(json!({"vars": [{"key": "DATABASE_URL", "is_secret": true}], "deployment": null})))
        .expect(1)
        .mount(&env.server)
        .await;
    let run = env.run(&["env", "set", "DATABASE_URL", "--from-db", "tienda-pg", "--service", "api"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(!run.all().contains("Pa55word"), "the password never reaches the output: {}", run.all());

    let dry = env.run(&["env", "set", "DATABASE_URL", "--from-db", "tienda-pg", "--service", "api", "--dry-run"]).await;
    assert_eq!(dry.code, 0);
    assert!(!dry.all().contains("Pa55word"), "nor a dry run: {}", dry.all());

    // `db url` only prints it with --reveal.
    let refused = env.run(&["db", "url", "tienda-pg"]).await;
    assert_eq!(refused.code, 2);
    assert!(refused.error()["message"].as_str().unwrap().contains("--reveal"));
    assert!(!refused.all().contains("Pa55word"));
    let url = env.run(&["db", "url", "tienda-pg", "--reveal"]).await;
    assert_eq!(url.code, 0);
    assert!(url.stdout.contains("Pa55word"));
    // `db credentials` masks without --reveal.
    let masked = env.run(&["db", "credentials", "tienda-pg"]).await;
    assert_eq!(masked.code, 0, "{}", masked.stderr);
    assert!(!masked.all().contains("Pa55word"), "{}", masked.stdout);
    assert_eq!(masked.json()["revealed"], false);
    let shown = env.run(&["db", "credentials", "tienda-pg", "--reveal"]).await;
    assert_eq!(shown.json()["password"], "Pa55word!");
}

#[tokio::test]
async fn env_import_marks_secret_keys_and_list_masks_them() {
    let env = Env::new().await.with_org().await.with_service().await;
    let env_path = format!("/organizations/{ORG}/deploy/services/{SVC}/env");
    Mock::given(method("GET"))
        .and(path(env_path.as_str()))
        .respond_with(ok(json!([{"key": "OLD", "value": "x", "is_secret": false}])))
        .mount(&env.server)
        .await;
    Mock::given(method("PUT"))
        .and(path(env_path.as_str()))
        .and(body_json(json!({"vars": [
            {"key": "A", "value": "1", "is_secret": false},
            {"key": "TOKEN", "value": "t0k", "is_secret": true},
        ]})))
        .respond_with(ok(json!({"vars": [{"key": "A", "value": "1", "is_secret": false},
            {"key": "TOKEN", "value": "t0k", "is_secret": true}], "deployment": null})))
        .expect(1)
        .mount(&env.server)
        .await;
    std::fs::write(env.cwd.path().join(".env"), "A=1\nTOKEN=t0k\n").unwrap();
    let run = env.run(&["env", "import", ".env", "--secret-key", "TOKEN", "--replace", "--service", "api"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["diff"]["removed"], json!(["OLD"]));
    assert!(!run.stdout.contains("t0k"), "secrets are masked in the output");
}

// ---------------------------------------------------------------- destructive

#[tokio::test]
async fn delete_needs_the_exact_name_and_dry_run_sends_nothing() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/projects")))
        .respond_with(ok(json!([{"id": PROJECT, "name": "Tienda"}])))
        .mount(&env.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/organizations/{ORG}/projects/{PROJECT}")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&env.server)
        .await;

    let none = env.run(&["project", "delete", "tienda"]).await;
    assert_eq!(none.code, 2);
    assert_eq!(none.error()["kind"], "confirmation_required");
    assert_eq!(none.error()["hint"], "--confirm Tienda");
    let wrong = env.run(&["project", "delete", "tienda", "--confirm", "tienda"]).await;
    assert_eq!(wrong.code, 2, "the name must match exactly");
    let dry = env.run(&["project", "delete", "tienda", "--dry-run"]).await;
    assert_eq!(dry.code, 0);
    let plan = dry.json();
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["method"], "DELETE");
    let done = env.run(&["project", "delete", "tienda", "--confirm", "Tienda"]).await;
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert_eq!(done.json()["deleted"], PROJECT);
}

#[tokio::test]
async fn create_if_not_exists_returns_the_existing_one() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/projects")))
        .respond_with(ok(json!([{"id": PROJECT, "name": "Tienda"}])))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/organizations/{ORG}/projects")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"success": true, "data": {"id": "new", "name": "Blog"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    let existing = env.run(&["project", "create", "tienda", "--if-not-exists"]).await;
    assert_eq!(existing.code, 0, "{}", existing.stderr);
    assert_eq!(existing.json()["id"], PROJECT);
    let created = env.run(&["project", "create", "Blog", "--if-not-exists"]).await;
    assert_eq!(created.code, 0);
    assert_eq!(created.json()["id"], "new");
}

// ---------------------------------------------------------------- deploy

#[tokio::test]
async fn deploy_wait_follows_the_deployment_until_live() {
    let env = Env::new().await.with_org().await.with_service().await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments")))
        .respond_with(ok(json!([])))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/deploy")))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true, "status_code": 202,
            "data": {"dispatched": false, "reason": "image_up_to_date", "deployment": {"id": "dep1", "status": "queued"}}})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/dep1")))
        .respond_with(ok(json!({"deployment": {"id": "dep1", "status": "deploying", "phase": "pulling"}})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/dep1")))
        .respond_with(ok(json!({"deployment": {"id": "dep1", "status": "live", "phase": "live"}})))
        .with_priority(2)
        .mount(&env.server)
        .await;
    let run = env.run(&["deploy", "api", "--wait", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["deployment"]["status"], "live");
}

#[tokio::test]
async fn deploy_wait_exits_7_when_the_deploy_fails_and_waits_for_the_build() {
    let env = Env::new().await.with_org().await.with_service().await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    // Before the dispatch there is one old deployment; the build then adds dep2.
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments")))
        .respond_with(ok(json!([{"id": "old"}])))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments")))
        .respond_with(ok(json!([{"id": "dep2"}, {"id": "old"}])))
        .with_priority(2)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/deploy")))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true,
            "data": {"dispatched": true, "reason": "new_commits", "deployment": null}})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/dep2")))
        .respond_with(ok(json!({"deployment": {"id": "dep2", "status": "failed", "phase": "health_check",
            "error_code": "health_check_failed", "error_message": "no responde en /health"}})))
        .mount(&env.server)
        .await;
    let run = env.run(&["deploy", "api", "--wait"]).await;
    assert_eq!(run.code, 7, "{}", run.stderr);
    let e = run.error();
    assert_eq!(e["kind"], "deploy_failed");
    assert!(e["hint"].as_str().unwrap().contains("fdt service diagnose api"));
}

#[tokio::test]
async fn deploy_dry_run_sends_nothing() {
    let env = Env::new().await.with_org().await.with_service().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&env.server)
        .await;
    let run = env.run(&["deploy", "api", "--dry-run"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["path"], format!("/organizations/{ORG}/deploy/services/{SVC}/deploy"));
}

// ---------------------------------------------------------------- misc

#[tokio::test]
async fn raw_api_fills_the_context_and_keeps_the_status() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/usage")))
        .respond_with(ok(json!({"projects": 1})))
        .mount(&env.server)
        .await;
    let run = env.run(&["api", "GET", "/organizations/{org}/usage"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["data"]["projects"], 1);
    let missing = env.run(&["api", "GET", "/nope"]).await;
    assert_eq!(missing.code, 5);
}

#[tokio::test]
async fn token_create_resolves_projects_and_prints_the_token_once() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/projects")))
        .respond_with(ok(json!([{"id": PROJECT, "name": "Tienda"}])))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/users/me/cli-tokens"))
        .and(body_json(json!({"name": "ci", "organization_id": ORG, "project_ids": [PROJECT],
            "level": "deploy", "expires_in_days": 30})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"success": true,
            "data": {"id": "t2", "token": "fdt_t2_new", "kind": "scoped"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    // Without --reveal or --output it refuses before creating anything.
    let refused = env.run(&["token", "create", "ci", "-p", "tienda"]).await;
    assert_eq!(refused.code, 2);
    let file = env.cwd.path().join("tok");
    let run = env.run(&["token", "create", "ci", "-p", "tienda", "--output", file.to_str().unwrap()]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(!run.all().contains("fdt_t2_new"), "with --output the token is not printed");
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "fdt_t2_new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[tokio::test]
async fn skills_install_into_the_repo() {
    let env = Env::new().await;
    std::fs::create_dir_all(env.cwd.path().join(".git")).unwrap();
    let list = env.run(&["skills"]).await;
    assert_eq!(list.code, 0, "{}", list.stderr);
    assert!(list.json()["skills"].as_array().unwrap().iter().any(|s| s["name"] == "fdt-basics"));
    let add = env.run(&["skills", "add", "fdt-basics"]).await;
    assert_eq!(add.code, 0, "{}", add.stderr);
    assert!(env.cwd.path().join(".claude/skills/fdt-basics/SKILL.md").is_file());
    assert!(env.cwd.path().join(".agents/skills/fdt-basics/SKILL.md").is_file());
    let remove = env.run(&["skills", "remove", "fdt-basics", "--agent", "claude"]).await;
    assert_eq!(remove.code, 0);
    assert!(!env.cwd.path().join(".claude/skills/fdt-basics").exists());
}

#[tokio::test]
async fn logout_revokes_and_forgets_the_saved_token() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/cli/whoami"))
        .respond_with(ok(json!({"user": {"email": "ana@acme.dev"}, "token": {"id": "t1", "kind": "session"}})))
        .mount(&env.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/cli/session"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&env.server)
        .await;
    let login = env.run_with(&["login", "--token", TOKEN], &[], None).await;
    assert_eq!(login.code, 0, "{}", login.stderr);
    let out = env.run_with(&["logout"], &[], None).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.json()["revoked"], true);
    let after = env.run_with(&["whoami"], &[], None).await;
    assert_eq!(after.code, 3);
}

// ---------------------------------------------------------------- second pass

/// A service that already has an image (so env changes restart it).
async fn with_live_service(env: Env) -> Env {
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services")))
        .respond_with(ok(json!([{"id": SVC, "slug": "api", "name": "API", "status": "live", "branch": "main",
            "current_deployment_id": "dep0", "latest_deployment": {"id": "dep0", "status": "live"}}])))
        .mount(&env.server)
        .await;
    env
}

#[tokio::test]
async fn project_rename_patches_the_org_route() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/projects")))
        .respond_with(ok(json!([{"id": PROJECT, "name": "Tienda"}])))
        .mount(&env.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("/organizations/{ORG}/projects/{PROJECT}")))
        .and(body_json(json!({"name": "Tienda 2", "description": "nueva"})))
        .respond_with(ok(json!({"id": PROJECT, "name": "Tienda 2"})))
        .expect(1)
        .mount(&env.server)
        .await;
    let none = env.run(&["project", "rename", "tienda"]).await;
    assert_eq!(none.code, 2);
    let run = env.run(&["project", "rename", "tienda", "Tienda 2", "--description", "nueva"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["name"], "Tienda 2");
}

#[tokio::test]
async fn rollback_to_a_deployment_uses_its_digest_and_waits() {
    let env = with_live_service(Env::new().await.with_org().await).await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    let digest = format!("sha256:{}", "a".repeat(64));
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments")))
        .respond_with(ok(json!([{"id": "3f2a9c1d-0000", "image_digest": digest}])))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/rollback")))
        .and(body_json(json!({"image_digest": digest})))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true,
            "data": {"id": "rb1", "status": "queued", "source": "rollback"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/rb1")))
        .respond_with(ok(json!({"deployment": {"id": "rb1", "status": "live"}})))
        .mount(&env.server)
        .await;
    let missing = env.run(&["rollback", "api"]).await;
    assert_eq!(missing.code, 2);
    let dry = env.run(&["rollback", "api", "--tag", "v1.2.0", "--dry-run"]).await;
    assert_eq!(dry.json()["body"]["image_tag"], "v1.2.0");
    let run = env.run(&["rollback", "api", "--to", "3f2a9c1d", "--wait"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["status"], "live");
}

#[tokio::test]
async fn service_restart_pause_and_resume() {
    let env = with_live_service(Env::new().await.with_org().await).await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("POST"))
        .and(path(format!("{base}/restart")))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true, "data": {"id": SVC, "status": "live"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/pause")))
        .respond_with(ok(json!({"id": SVC, "status": "paused"})))
        .expect(1)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/resume")))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true, "data": {"id": "res1", "status": "queued", "source": "resume"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/res1")))
        .respond_with(ok(json!({"deployment": {"id": "res1", "status": "live"}})))
        .mount(&env.server)
        .await;
    assert_eq!(env.run(&["service", "restart", "api"]).await.code, 0);
    let unconfirmed = env.run(&["service", "pause", "api"]).await;
    assert_eq!(unconfirmed.code, 2);
    assert_eq!(unconfirmed.error()["kind"], "confirmation_required");
    let dry = env.run(&["service", "pause", "api", "--dry-run"]).await;
    assert_eq!(dry.json()["path"], format!("{base}/pause"));
    let paused = env.run(&["service", "pause", "api", "--confirm", "api"]).await;
    assert_eq!(paused.code, 0, "{}", paused.stderr);
    let resumed = env.run(&["service", "resume", "api", "--wait"]).await;
    assert_eq!(resumed.code, 0, "{}", resumed.stderr);
    assert_eq!(resumed.json()["status"], "live");
}

#[tokio::test]
async fn logs_send_since_and_until_to_core() {
    let env = Env::new().await.with_org().await.with_service().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services/{SVC}/logs")))
        .and(query_param("since", "2026-11-23T10:00:00Z"))
        .and(query_param("until", "2026-11-23T11:00:00Z"))
        .respond_with(ok(json!({"lines": [{"at": "2026-11-23T10:30:00Z", "stream": "stdout", "text": "hola"}]})))
        .expect(1)
        .mount(&env.server)
        .await;
    let run = env.run(&["logs", "api", "--since", "2026-11-23T10:00:00Z", "--until", "2026-11-23T11:00:00Z"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["lines"][0]["text"], "hola");
    let bad = env.run(&["logs", "api", "--since", "2026-11-23T11:00:00Z", "--until", "2026-11-23T10:00:00Z"]).await;
    assert_eq!(bad.code, 2);
}

#[tokio::test]
async fn deploy_rebuild_sends_the_body_and_dry_run_predicts() {
    let env = Env::new().await.with_org().await.with_service().await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("POST"))
        .and(path(format!("{base}/deploy")))
        .and(body_json(json!({"rebuild": true})))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true,
            "data": {"dispatched": true, "reason": "rebuild", "deployment": null}})))
        .expect(1)
        .mount(&env.server)
        .await;
    // The mocked service has no image: Core would run the workflow (no_image).
    let dry = env.run(&["deploy", "api", "--dry-run"]).await;
    assert_eq!(dry.json()["prediction"]["would"], "workflow");
    assert_eq!(dry.json()["prediction"]["reason"], "no_image");
    let run = env.run(&["deploy", "api", "--rebuild"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["reason"], "rebuild");
}

#[tokio::test]
async fn deploy_dry_run_on_a_live_service_says_it_cannot_tell() {
    let env = with_live_service(Env::new().await.with_org().await).await;
    let dry = env.run(&["deploy", "api", "--dry-run"]).await;
    let p = dry.json()["prediction"].clone();
    assert_eq!(p["would"], "redeploy_or_workflow");
    assert_eq!(p["certain"], false);
}

#[tokio::test]
async fn wait_timeout_exits_8() {
    let env = Env::new().await.with_org().await.with_service().await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("POST"))
        .and(path(format!("{base}/redeploy")))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true, "data": {"id": "dep9", "status": "queued"}})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/dep9")))
        .respond_with(ok(json!({"deployment": {"id": "dep9", "status": "deploying"}})))
        .mount(&env.server)
        .await;
    let run = env.run(&["redeploy", "api", "--wait", "--timeout", "1s"]).await;
    assert_eq!(run.code, 8, "{}", run.stderr);
    assert_eq!(run.error()["kind"], "timeout");
}

#[tokio::test]
async fn parallel_deploys_are_flagged_as_ambiguous() {
    let env = Env::new().await.with_org().await.with_service().await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments")))
        .respond_with(ok(json!([])))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments")))
        .respond_with(ok(json!([{"id": "mine"}, {"id": "theirs"}])))
        .with_priority(2)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/deploy")))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"success": true,
            "data": {"dispatched": true, "reason": "new_commits", "deployment": null}})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/deployments/mine")))
        .respond_with(ok(json!({"deployment": {"id": "mine", "status": "live"}})))
        .mount(&env.server)
        .await;
    let run = env.run(&["deploy", "api", "--wait"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let out = run.json();
    assert_eq!(out["ambiguous"], true);
    assert_eq!(out["candidates"].as_array().unwrap().len(), 2);
    assert!(run.stderr.contains("en paralelo"));
}

#[tokio::test]
async fn cli_token_scopes_exit_4_with_what_is_needed() {
    let env = Env::new().await.with_org().await.with_service().await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("POST"))
        .and(path(format!("{base}/restart")))
        .respond_with(err(403, "cli.token_level"))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/apps")))
        .respond_with(ok(json!([{"id": "app1", "repository": {"full_name": "acme/api"}}])))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/organizations/{ORG}/deploy/services")))
        .respond_with(err(403, "cli.token_scope"))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/members")))
        .respond_with(err(403, "cli.token_scope"))
        .mount(&env.server)
        .await;
    let level = env.run(&["service", "restart", "api"]).await;
    assert_eq!(level.code, 4);
    assert_eq!(level.error()["scope"], "cli.token_level");
    assert!(level.error()["hint"].as_str().unwrap().contains("--level admin"));
    let scoped = env.run(&["service", "create", "web", "--repo", "acme/api"]).await;
    assert_eq!(scoped.code, 4);
    let message = scoped.error()["message"].as_str().unwrap().to_owned();
    assert!(message.contains("crear servicios") && message.contains("project_ids"), "{message}");
    let members = env.run(&["org", "members"]).await;
    assert_eq!(members.code, 4);
    assert!(members.error()["message"].as_str().unwrap().contains("ver los miembros"));
}

#[tokio::test]
async fn login_when_core_is_not_configured_explains_it() {
    let env = Env::new().await;
    Mock::given(method("POST"))
        .and(path("/cli/device/code"))
        .respond_with(err(503, "cli.not_configured"))
        .mount(&env.server)
        .await;
    let run = env.run_with(&["login"], &[], None).await;
    assert_eq!(run.code, 1);
    assert_eq!(run.error()["scope"], "cli.not_configured");
    assert!(run.error()["hint"].as_str().unwrap().contains("--with-token"));
}

#[tokio::test]
async fn slow_down_uses_the_interval_core_sends() {
    let env = Env::new().await;
    mount_device_code(&env.server).await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"success": false,
            "error": {"code": 400, "message": "slow down", "scope": "cli.slow_down", "meta": {"interval": 7}}})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cli/device/token"))
        .respond_with(ok(json!({"token": TOKEN, "token_id": "t", "kind": "session", "expires_at": "2027-01-01T00:00:00Z",
            "user": {"id": "u", "email": "a@b.c", "name": "A"}})))
        .with_priority(2)
        .mount(&env.server)
        .await;
    let started = std::time::Instant::now();
    let run = env.run_with(&["login"], &[], None).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    // 1 unit before the first poll + 7 after slow_down, 5 ms each.
    assert!(started.elapsed() >= std::time::Duration::from_millis(40));
}

#[tokio::test]
async fn env_secrets_from_env_and_argv_warning_and_redeploy_flag() {
    let env = with_live_service(Env::new().await.with_org().await).await;
    let env_path = format!("/organizations/{ORG}/deploy/services/{SVC}/env");
    Mock::given(method("GET"))
        .and(path(env_path.as_str()))
        .respond_with(ok(json!([])))
        .mount(&env.server)
        .await;
    Mock::given(method("PUT"))
        .and(path(env_path.as_str()))
        .and(body_json(json!({"vars": [{"key": "STRIPE_KEY", "value": "sk_from_env", "is_secret": true}]})))
        .respond_with(ok(json!({"vars": [{"key": "STRIPE_KEY", "is_secret": true}],
            "deployment": {"id": "envdep", "status": "queued"}, "deployment_skipped_reason": null})))
        .expect(1)
        .mount(&env.server)
        .await;
    let dry = env.run(&["env", "set", "A=1", "--service", "api", "--dry-run"]).await;
    assert_eq!(dry.code, 0);
    assert_eq!(dry.json()["redeploys"], true);
    let argv = env.run(&["env", "set", "TOK=abc", "--secret", "--service", "api", "--dry-run"]).await;
    assert!(argv.stderr.contains("historial"), "{}", argv.stderr);
    assert!(!argv.stdout.contains("abc"));
    let run = env
        .run_with(
            &["env", "set", "STRIPE_KEY", "--secret", "--from-env", "MY_STRIPE", "--service", "api"],
            &[("FDT_TOKEN", TOKEN), ("MY_STRIPE", "sk_from_env")],
            None,
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(!run.stderr.contains("historial"));
    assert!(!run.all().contains("sk_from_env"));
    assert_eq!(run.json()["redeploys"], true);
    assert_eq!(run.json()["deployment"]["id"], "envdep");
    let missing = env.run(&["env", "set", "K", "--from-env", "NOT_SET_ANYWHERE", "--service", "api"]).await;
    assert_eq!(missing.code, 2);
}

#[tokio::test]
async fn service_setup_is_read_only_and_rotation_needs_confirm() {
    let env = with_live_service(Env::new().await.with_org().await).await;
    let base = format!("/organizations/{ORG}/deploy/services/{SVC}");
    Mock::given(method("GET"))
        .and(path(format!("{base}/setup")))
        .respond_with(ok(json!({"workflow_yaml": "name: x\n", "workflow_path": ".github/workflows/foundathyon.yml",
            "secret_name": "FOUNDATHYON_REGISTRY_TOKEN", "registry_token": null})))
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/registry-token")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"success": true,
            "data": {"id": "r1", "secret": "fdr_r1_SECRETVALUE", "username": "acme"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/workflow")))
        .and(body_json(json!({"workflow_mode": "commit"})))
        .respond_with(ok(json!({"service": {}, "workflow": {"mode": "commit", "status": "installed", "error": null}})))
        .expect(1)
        .mount(&env.server)
        .await;
    let setup = env.run(&["service", "setup", "api", "--write-workflow"]).await;
    assert_eq!(setup.code, 0, "{}", setup.stderr);
    assert!(env.cwd.path().join(".github/workflows/foundathyon.yml").is_file());
    let install = env.run(&["service", "install-workflow", "api", "--mode", "commit"]).await;
    assert_eq!(install.code, 0, "{}", install.stderr);
    assert!(install.stderr.contains("git pull"), "{}", install.stderr);
    let unconfirmed = env.run(&["service", "rotate-registry-token", "api", "--reveal"]).await;
    assert_eq!(unconfirmed.code, 2);
    let no_dest = env.run(&["service", "rotate-registry-token", "api", "--confirm", "api"]).await;
    assert_eq!(no_dest.code, 2);
    let file = env.cwd.path().join("fdr");
    let rotated = env
        .run(&["service", "rotate-registry-token", "api", "--confirm", "api", "--output", file.to_str().unwrap()])
        .await;
    assert_eq!(rotated.code, 0, "{}", rotated.stderr);
    assert!(!rotated.all().contains("SECRETVALUE"));
    assert!(std::fs::read_to_string(&file).unwrap().contains("fdr_r1_SECRETVALUE"));
}

#[tokio::test]
async fn link_checks_that_the_org_exists_before_writing() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/organizations/gone"))
        .respond_with(err(404, "access.organization.not_found"))
        .mount(&env.server)
        .await;
    std::fs::write(env.cwd.path().join(".fdt.toml"), "org = \"gone\"\norg_name = \"Vieja\"\n").unwrap();
    let sub = env.cwd.path().join("app");
    std::fs::create_dir_all(sub.join(".git")).unwrap();
    let run = env.run_in(&sub, &["link"], &[("FDT_TOKEN", TOKEN)], None).await;
    assert_eq!(run.code, 5, "{}", run.stderr);
    assert!(!sub.join(".fdt.toml").exists());
}

#[tokio::test]
async fn scoped_lists_say_they_are_filtered() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/services")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": [],
            "meta": {"limits": {}, "cli_filtered": true}})))
        .mount(&env.server)
        .await;
    let run = env.run(&["service", "list"]).await;
    assert_eq!(run.code, 0);
    assert!(run.stderr.contains("filtrada"), "{}", run.stderr);
}

// ------------------------------------------- found against a real Core + Postgres

#[tokio::test]
async fn db_credentials_without_reveal_does_not_fetch_the_password() {
    // A read token gets 403 cli.token_level on …/credentials: without --reveal
    // fdt shows what the base itself carries and never asks for the secret.
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/databases")))
        .respond_with(ok(json!({"databases": [{"id": DB, "name": "tienda-pg", "username": "u", "db_name": "app",
            "port": 5432, "internal_host": "db-1", "internal_connection_string": "postgres://u@db-1:5432/app"}]})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/databases/{DB}/credentials")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"success": false, "error": {"code": 403,
            "scope": "cli.token_level", "message": "This read returns a secret and needs a token of level deploy or higher"}})))
        .expect(1)
        .mount(&env.server)
        .await;
    let masked = env.run(&["db", "credentials", "tienda-pg"]).await;
    assert_eq!(masked.code, 0, "{}", masked.stderr);
    let out = masked.json();
    assert_eq!(out["revealed"], false);
    assert_eq!(out["username"], "u");
    assert_eq!(out["internal_connection_string"], "postgres://u@db-1:5432/app");
    // --reveal does ask (the one call the mock expects) and says which level it needs.
    let shown = env.run(&["db", "credentials", "tienda-pg", "--reveal"]).await;
    assert_eq!(shown.code, 4);
    assert!(shown.error()["hint"].as_str().unwrap().contains("--level deploy"), "{}", shown.stderr);
}

#[tokio::test]
async fn token_revoke_reports_the_cascade_and_takes_an_id_without_listing() {
    let env = Env::new().await;
    let id = "55555555-5555-4555-8555-555555555555";
    Mock::given(method("DELETE"))
        .and(path(format!("/users/me/cli-tokens/{id}")))
        .respond_with(ok(json!({"id": id, "kind": "session", "cascade_revoked": 3})))
        .expect(1)
        .mount(&env.server)
        .await;
    // No GET /users/me/cli-tokens mock: a scoped token cannot list, but can revoke itself by id.
    let run = env.run(&["token", "revoke", id]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["cascade_revoked"], 3);
    assert!(run.stderr.contains("3 token(s)"), "{}", run.stderr);
}

#[tokio::test]
async fn another_org_id_goes_to_core_and_exits_4() {
    // A scoped token only lists its own org; an id it does not list is
    // Core's call (403 cli.token_scope), not "not found".
    let env = Env::new().await.with_org().await;
    let other = "66666666-6666-4666-8666-666666666666";
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{other}/projects")))
        .respond_with(err(403, "cli.token_scope"))
        .mount(&env.server)
        .await;
    let run = env.run(&["project", "list", "--org", other]).await;
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert_eq!(run.error()["scope"], "cli.token_scope");
    // A name it does not list is still not found, with a hint about scoped tokens.
    let name = env.run(&["project", "list", "--org", "Beta"]).await;
    assert_eq!(name.code, 5);
    assert!(name.error()["hint"].as_str().unwrap().contains("scoped"));
}

#[tokio::test]
async fn service_create_dry_run_shows_the_body_it_sends_and_the_real_url() {
    let env = Env::new().await.with_org().await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/apps")))
        .respond_with(ok(json!([{"id": "app1", "slug": "api", "repository": {"full_name": "acme/api"}}])))
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/organizations/{ORG}/deploy/subdomain")))
        .respond_with(ok(json!({"suffix": "acme", "domain": "foundathyon.dev"})))
        .mount(&env.server)
        .await;
    let run = env.run(&["service", "create", "web", "--repo", "acme/api", "--dry-run"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.json()["body"].get("env").is_none(), "no env → no env field: {}", run.stdout);
    // POST …/deploy/services keeps the slug: https://<slug>.<domain>.
    assert!(run.stderr.contains("https://web.foundathyon.dev"), "{}", run.stderr);
}
