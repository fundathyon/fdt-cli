//! `.env` files and the merge `fdt env` does before the PUT that replaces the
//! whole set (the API has no per-variable route).

use crate::failure::{Kind, fail};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Valid variable name: letters, digits and `_`, not starting with a digit.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses `KEY=VALUE` lines: `export `, comments, single and double quotes
/// (double quotes understand `\n`, `\"` and `\\`).
pub fn parse(text: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            return Err(fail(Kind::Usage, format!("línea {}: falta «=»", n + 1)));
        };
        let key = key.trim();
        if !valid_key(key) {
            return Err(fail(Kind::Usage, format!("línea {}: nombre de variable inválido «{key}»", n + 1)));
        }
        let value = value.trim();
        let value = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            let mut s = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some(other) => s.push(other),
                        None => s.push('\\'),
                    }
                } else {
                    s.push(c);
                }
            }
            s
        } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            inner.to_owned()
        } else {
            // Unquoted: an inline comment starts at " #".
            value.split(" #").next().unwrap_or("").trim_end().to_owned()
        };
        out.push((key.to_owned(), value));
    }
    Ok(out)
}

/// One `KEY=value` line, quoted when needed.
pub fn line(key: &str, value: &str) -> String {
    let plain = !value.is_empty()
        && value.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@+,%".contains(c));
    if plain {
        format!("{key}={value}")
    } else {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        format!("{key}=\"{escaped}\"")
    }
}

/// A variable as the API stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub key: String,
    /// `None` for an existing secret: the API keeps its value.
    pub value: Option<String>,
    pub is_secret: bool,
}

pub fn from_api(vars: &Value) -> Vec<Var> {
    vars.as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(Var {
                key: v["key"].as_str()?.to_owned(),
                value: v["value"].as_str().map(str::to_owned),
                is_secret: v["is_secret"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// What changes between the current set and the new one (keys only).
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Diff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

/// Applies `set` (key → value, secret?) and `unset` to `current`.
/// A variable that was secret stays secret. With `replace`, keys not in `set` go away.
pub fn merge(
    current: &[Var],
    set: &[(String, String, bool)],
    unset: &[String],
    replace: bool,
) -> (Vec<Var>, Diff) {
    let mut map: BTreeMap<String, Var> = current.iter().map(|v| (v.key.clone(), v.clone())).collect();
    let mut diff = Diff::default();
    if replace {
        let keep: Vec<&String> = set.iter().map(|(k, _, _)| k).collect();
        let gone: Vec<String> = map.keys().filter(|k| !keep.contains(k)).cloned().collect();
        for k in gone {
            map.remove(&k);
            diff.removed.push(k);
        }
    }
    for (key, value, secret) in set {
        match map.get_mut(key) {
            Some(existing) => {
                let is_secret = *secret || existing.is_secret;
                // A secret's current value is unknown: setting it always counts as a change.
                if existing.value.as_deref() == Some(value.as_str()) && existing.is_secret == is_secret {
                    diff.unchanged.push(key.clone());
                } else {
                    diff.changed.push(key.clone());
                }
                existing.value = Some(value.clone());
                existing.is_secret = is_secret;
            }
            None => {
                map.insert(key.clone(), Var { key: key.clone(), value: Some(value.clone()), is_secret: *secret });
                diff.added.push(key.clone());
            }
        }
    }
    for key in unset {
        if map.remove(key).is_some() {
            diff.removed.push(key.clone());
        }
    }
    (map.into_values().collect(), diff)
}

/// The body of `PUT …/env`: a kept secret goes without `value`.
pub fn body(vars: &[Var]) -> Value {
    json!({
        "vars": vars.iter().map(|v| match &v.value {
            Some(value) => json!({"key": v.key, "value": value, "is_secret": v.is_secret}),
            None => json!({"key": v.key, "is_secret": v.is_secret}),
        }).collect::<Vec<_>>()
    })
}

/// A body safe to print: secret values hidden.
pub fn masked_body(vars: &[Var]) -> Value {
    json!({
        "vars": vars.iter().map(|v| json!({
            "key": v.key,
            "value": if v.is_secret { Value::String("••••••".into()) } else { json!(v.value) },
            "is_secret": v.is_secret,
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(k: &str, v: Option<&str>, s: bool) -> Var {
        Var { key: k.into(), value: v.map(Into::into), is_secret: s }
    }

    #[test]
    fn parses_dotenv() {
        let parsed = parse("# c\nexport A=1\nB=\"x y\\n\"\nC='z #'\nD=v # comment\nE=\n").unwrap();
        assert_eq!(
            parsed,
            vec![
                ("A".into(), "1".into()),
                ("B".into(), "x y\n".into()),
                ("C".into(), "z #".into()),
                ("D".into(), "v".into()),
                ("E".into(), "".into()),
            ]
        );
        assert!(parse("1A=x").is_err());
        assert!(parse("NOEQUALS").is_err());
        assert_eq!(line("A", "x y"), "A=\"x y\"");
        assert_eq!(line("A", "postgres://h:5432/db"), "A=postgres://h:5432/db");
    }

    #[test]
    fn merge_keeps_secrets_and_reports_changes() {
        let current = vec![var("API_KEY", None, true), var("MODE", Some("dev"), false), var("OLD", Some("1"), false)];
        let (vars, diff) = merge(
            &current,
            &[("MODE".into(), "prod".into(), false), ("NEW".into(), "v".into(), false)],
            &["OLD".into(), "MISSING".into()],
            false,
        );
        assert_eq!(diff.added, ["NEW"]);
        assert_eq!(diff.changed, ["MODE"]);
        assert_eq!(diff.removed, ["OLD"]);
        let body = body(&vars);
        let api_key = body["vars"].as_array().unwrap().iter().find(|v| v["key"] == "API_KEY").unwrap();
        assert!(api_key.get("value").is_none(), "an untouched secret keeps its server value");
        assert_eq!(api_key["is_secret"], true);
    }

    #[test]
    fn setting_a_secret_keeps_it_secret_and_masks_it() {
        let current = vec![var("TOKEN", None, true)];
        let (vars, diff) = merge(&current, &[("TOKEN".into(), "s3cr3t".into(), false)], &[], false);
        assert_eq!(diff.changed, ["TOKEN"]);
        assert!(vars[0].is_secret);
        assert!(!masked_body(&vars).to_string().contains("s3cr3t"));
    }

    #[test]
    fn replace_drops_missing_keys() {
        let current = vec![var("A", Some("1"), false), var("B", Some("2"), false)];
        let (vars, diff) = merge(&current, &[("A".into(), "1".into(), false)], &[], true);
        assert_eq!(diff.removed, ["B"]);
        assert_eq!(diff.unchanged, ["A"]);
        assert_eq!(vars.len(), 1);
    }
}
