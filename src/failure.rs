//! Errors a harness can act on without parsing prose: each failure has a
//! stable `kind`, a process exit code, the API's `error.scope` when there is
//! one, and the command that fixes it when we know it.
//!
//! ```json
//! {"ok": false, "error": {"kind": "quota", "exit_code": 6, "scope": "quota.deploy.services",
//!   "status": 403, "message": "…", "hint": "fdt org usage", "details": {…}}}
//! ```
//!
//! Exit codes: 0 ok, 1 error, 2 usage, 3 no session, 4 forbidden, 5 not found,
//! 6 plan quota, 7 deploy failed, 8 `--wait` timed out.

use serde::Serialize;
use serde_json::{Value, json};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Anything without a more specific kind.
    Error,
    /// The API answered with an error that has no more specific kind.
    Api,
    /// DNS, connection or timeout talking to the API.
    Network,
    /// `--wait` gave up before the operation finished.
    Timeout,
    /// A person answered no.
    Cancelled,
    /// Wrong flags or arguments.
    Usage,
    /// A required value is missing and there is no terminal to ask for it.
    MissingInput,
    /// A name matches several resources.
    Ambiguous,
    /// A destructive command without the right `--confirm`.
    ConfirmationRequired,
    /// No token, or the API rejected it (401).
    Unauthenticated,
    /// The device login expired or was denied.
    LoginFailed,
    /// The token or the user may not do this (403).
    Forbidden,
    /// The resource does not exist (404 or no local match).
    NotFound,
    /// The organization's plan does not allow it (`quota.*`).
    Quota,
    /// A deploy followed with `--wait` failed or was cancelled.
    DeployFailed,
}

impl Kind {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Error | Self::Api | Self::Network | Self::Cancelled => 1,
            Self::Usage | Self::MissingInput | Self::Ambiguous | Self::ConfirmationRequired => 2,
            Self::Unauthenticated | Self::LoginFailed => 3,
            Self::Forbidden => 4,
            Self::NotFound => 5,
            Self::Quota => 6,
            Self::DeployFailed => 7,
            Self::Timeout => 8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: Kind,
    pub message: String,
    /// `error.scope` of the API answer.
    pub scope: Option<String>,
    /// HTTP status of the API answer.
    pub status: Option<u16>,
    /// The command that fixes it, when known.
    pub hint: Option<String>,
    pub details: Option<Value>,
}

impl Failure {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            scope: None,
            status: None,
            hint: None,
            details: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// An error of a given kind, to return with `?` or `Err(...)`.
pub fn fail(kind: Kind, message: impl Into<String>) -> anyhow::Error {
    Failure::new(kind, message).into()
}

/// An error of a given kind with the command that fixes it.
pub fn fail_hint(kind: Kind, message: impl Into<String>, hint: impl Into<String>) -> anyhow::Error {
    Failure::new(kind, message).hint(hint).into()
}

/// The failure inside an error chain, if any.
pub fn failure_of(error: &anyhow::Error) -> Option<&Failure> {
    error.chain().find_map(|e| e.downcast_ref::<Failure>())
}

/// The JSON a failed command prints, and its exit code. The message keeps
/// anyhow's context chain and never carries a token.
pub fn report(error: &anyhow::Error) -> (Value, i32) {
    let failure = failure_of(error);
    let kind = failure.map_or(Kind::Error, |f| f.kind);
    let message = crate::secrets::redact(
        &error
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": "),
    );
    let mut body = json!({"kind": kind, "exit_code": kind.exit_code(), "message": message});
    if let Some(f) = failure {
        if let Some(scope) = &f.scope {
            body["scope"] = json!(scope);
        }
        if let Some(status) = f.status {
            body["status"] = json!(status);
        }
        if let Some(hint) = &f.hint {
            body["hint"] = json!(hint);
        }
        if let Some(details) = &f.details {
            body["details"] = details.clone();
        }
    }
    (json!({"ok": false, "error": body}), kind.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn report_keeps_kind_scope_hint_and_context() {
        let mut f = Failure::new(Kind::Quota, "límite").hint("fdt org usage");
        f.scope = Some("quota.deploy.services".into());
        f.status = Some(403);
        let error = Err::<(), _>(anyhow::Error::from(f))
            .context("creando el servicio")
            .unwrap_err();
        let (json, code) = report(&error);
        assert_eq!(code, 6);
        assert_eq!(json["error"]["kind"], "quota");
        assert_eq!(json["error"]["scope"], "quota.deploy.services");
        assert_eq!(json["error"]["hint"], "fdt org usage");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("creando el servicio: ")
        );
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(Kind::Error.exit_code(), 1);
        assert_eq!(Kind::Usage.exit_code(), 2);
        assert_eq!(Kind::Unauthenticated.exit_code(), 3);
        assert_eq!(Kind::Forbidden.exit_code(), 4);
        assert_eq!(Kind::NotFound.exit_code(), 5);
        assert_eq!(Kind::Quota.exit_code(), 6);
        assert_eq!(Kind::DeployFailed.exit_code(), 7);
        assert_eq!(Kind::Timeout.exit_code(), 8);
    }

    #[test]
    fn errors_never_carry_a_token() {
        let (json, _) = report(&anyhow::anyhow!("falló con fdt_abc123_s3cr3tvalue"));
        let message = json["error"]["message"].as_str().unwrap();
        assert!(!message.contains("s3cr3t"), "{message}");
    }
}
