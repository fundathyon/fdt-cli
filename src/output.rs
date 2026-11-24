//! How results are printed: JSON on stdout with `--json` or without a
//! terminal (the same shape as the API's `data`), a table or text otherwise.

use anyhow::Result;
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub struct Out {
    pub json: bool,
}

impl Out {
    /// Prints `data` as JSON, or `human()` when a person is reading.
    pub fn print(&self, data: &Value, human: impl FnOnce() -> String) -> Result<()> {
        if self.json {
            print_json(data)
        } else {
            let text = human();
            if !text.is_empty() {
                println!("{text}");
            }
            Ok(())
        }
    }
}

pub fn print_json(data: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(data)?);
    Ok(())
}

/// A string field, or `-`.
pub fn s(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(text) if !text.is_empty() => text.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => "-".to_owned(),
    }
}

/// A nested string field (`a.b.c`), or `-`.
pub fn path(value: &Value, dotted: &str) -> String {
    let mut current = value;
    for key in dotted.split('.') {
        current = &current[key];
    }
    match current {
        Value::String(text) if !text.is_empty() => text.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => "-".to_owned(),
    }
}

/// `2026-11-23T10:00:00Z` → `2026-11-23 10:00`.
pub fn when(value: &Value, key: &str) -> String {
    let raw = s(value, key);
    match chrono::DateTime::parse_from_rfc3339(&raw) {
        Ok(t) => t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
        Err(_) => raw,
    }
}

/// Indented `key: value` lines for a detail view.
pub fn fields(rows: &[(&str, String)]) -> String {
    let width = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let c = crate::ui::Colors::stdout();
    rows.iter()
        .map(|(k, v)| format!("{} {v}", c.dim(&format!("{k:<width$}"))))
        .collect::<Vec<_>>()
        .join("\n")
}
