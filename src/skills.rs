//! `fdt skills`: installs the agent skills of this repo's `skills/<name>/`
//! folder (embedded in the binary at build time, or read from `--from <dir>`)
//! into a repo or the user's home, for Claude Code (`.claude/skills`) and
//! Codex/Cursor/Copilot (`.agents/skills`).

use crate::context::Ctx;
use crate::failure::{Kind, fail};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/skills.rs"));
}

pub const MARKER: &str = ".fdt-skill.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
    Copilot,
}

impl Agent {
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "copilot" => Ok(Self::Copilot),
            other => Err(fail(
                Kind::Usage,
                format!("Agente desconocido: {other}. Usa claude, codex, cursor o copilot."),
            )),
        }
    }

    pub fn dir(self) -> &'static str {
        match self {
            Self::Claude => ".claude/skills",
            Self::Codex | Self::Cursor | Self::Copilot => ".agents/skills",
        }
    }
}

pub enum Action {
    List,
    Add(Vec<String>),
    Remove(Vec<String>),
    Update,
}

pub struct Options {
    pub global: bool,
    pub agents: Option<String>,
    pub from: Option<PathBuf>,
}

/// skill → (relative path → bytes)
type Catalog = BTreeMap<String, BTreeMap<String, Vec<u8>>>;

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

fn safe_rel(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(|p| !p.is_empty() && p != ".." && p != ".")
}

fn embedded_catalog() -> Catalog {
    let mut catalog = Catalog::new();
    for (skill, path, bytes) in embedded::EMBEDDED {
        if valid_name(skill) && safe_rel(path) {
            catalog.entry((*skill).to_owned()).or_default().insert((*path).to_owned(), bytes.to_vec());
        }
    }
    catalog
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if safe_rel(&rel) {
                out.insert(rel, std::fs::read(&path)?);
            }
        }
    }
    Ok(())
}

fn local_catalog(from: &Path) -> Result<Catalog> {
    let root = if from.join("skills").is_dir() { from.join("skills") } else { from.to_path_buf() };
    if !root.is_dir() {
        return Err(fail(Kind::Usage, format!("{} no es una carpeta de skills.", from.display())));
    }
    let mut catalog = Catalog::new();
    for entry in std::fs::read_dir(&root)? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() && valid_name(&name) && path.join("SKILL.md").is_file() {
            let mut files = BTreeMap::new();
            walk(&path, &path, &mut files)?;
            catalog.insert(name, files);
        }
    }
    Ok(catalog)
}

fn description(files: &BTreeMap<String, Vec<u8>>) -> Option<String> {
    let text = String::from_utf8_lossy(files.get("SKILL.md")?).into_owned();
    let front = text.strip_prefix("---")?.split("\n---").next()?.to_owned();
    front.lines().find_map(|l| l.strip_prefix("description:").map(|d| d.trim().trim_matches('"').to_owned()))
}

fn destinations(base: &Path, agents: &[Agent]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = agents.iter().map(|a| base.join(a.dir())).collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

fn read_marker(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(MARKER)).ok()?).ok()
}

fn install(dir: &Path, name: &str, files: &BTreeMap<String, Vec<u8>>, source: &str) -> Result<()> {
    let target = dir.join(name);
    if target.exists() {
        if read_marker(&target).is_none() {
            return Err(fail(
                Kind::Error,
                format!("{} existe y no lo instaló fdt: no se toca.", target.display()),
            ));
        }
        std::fs::remove_dir_all(&target)?;
    }
    for (rel, bytes) in files {
        let path = target.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes).with_context(|| format!("no se pudo escribir {}", path.display()))?;
    }
    let marker = json!({
        "source": source, "version": crate::VERSION, "skill": name,
        "installed_at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(target.join(MARKER), serde_json::to_string_pretty(&marker)?)?;
    Ok(())
}

fn remove_if_empty(dir: &Path) {
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
        let _ = std::fs::remove_dir(dir);
    }
}

pub fn run(ctx: &Ctx, action: Action, options: Options) -> Result<()> {
    let (catalog, source) = match &options.from {
        Some(from) => (local_catalog(from)?, from.display().to_string()),
        None => (embedded_catalog(), format!("fdt {}", crate::VERSION)),
    };
    let agents = match &options.agents {
        Some(list) => list.split(',').filter(|s| !s.trim().is_empty()).map(Agent::parse).collect::<Result<Vec<_>>>()?,
        None => vec![Agent::Claude, Agent::Codex],
    };
    let base = if options.global {
        directories::BaseDirs::new().context("sin directorio de usuario")?.home_dir().to_path_buf()
    } else {
        ctx.cwd.ancestors().find(|d| d.join(".git").exists()).unwrap_or(&ctx.cwd).to_path_buf()
    };
    let dirs = destinations(&base, &agents);
    let installed = |name: &str| -> Vec<String> {
        dirs.iter()
            .filter(|d| d.join(name).join("SKILL.md").is_file())
            .map(|d| d.join(name).display().to_string())
            .collect()
    };

    let names = |requested: Vec<String>| -> Result<Vec<String>> {
        if requested.iter().any(|r| r == "all") {
            return Ok(catalog.keys().cloned().collect());
        }
        for r in &requested {
            if !catalog.contains_key(r) {
                return Err(crate::failure::Failure::new(Kind::NotFound, format!("No hay ninguna skill «{r}».")).hint("fdt skills list").into());
            }
        }
        Ok(requested)
    };

    let (verb, done): (&str, Vec<Value>) = match action {
        Action::List => {
            let list: Vec<Value> = catalog
                .iter()
                .map(|(name, files)| json!({
                    "name": name, "description": description(files), "installed": installed(name),
                }))
                .collect();
            let data = json!({"source": source, "destinations": dirs, "skills": list});
            return ctx.out.print(&data, || {
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|s| vec![
                        crate::output::s(s, "name"),
                        if s["installed"].as_array().is_some_and(|a| !a.is_empty()) { "sí".into() } else { "no".into() },
                        crate::output::s(s, "description").chars().take(80).collect(),
                    ])
                    .collect();
                crate::ui::table(&["skill", "instalada", "descripción"], &rows)
            });
        }
        Action::Add(requested) => {
            let mut done = Vec::new();
            for name in names(requested)? {
                for dir in &dirs {
                    install(dir, &name, &catalog[&name], &source)?;
                    done.push(json!({"skill": name, "path": dir.join(&name)}));
                }
            }
            ("instaladas", done)
        }
        Action::Update => {
            let mut done = Vec::new();
            for (name, files) in &catalog {
                for dir in &dirs {
                    if read_marker(&dir.join(name)).is_some() {
                        install(dir, name, files, &source)?;
                        done.push(json!({"skill": name, "path": dir.join(name)}));
                    }
                }
            }
            ("actualizadas", done)
        }
        Action::Remove(requested) => {
            // `all` is every skill fdt installed here (its marker), also ones
            // no longer in the catalog. A name that is neither in the catalog
            // nor installed is an error (exit 5), not a silent no-op.
            let fdt_installed = |name: &str| dirs.iter().any(|d| read_marker(&d.join(name)).is_some());
            let names: Vec<String> = if requested.iter().any(|r| r == "all") {
                let mut all: Vec<String> = dirs
                    .iter()
                    .filter_map(|d| std::fs::read_dir(d).ok())
                    .flatten()
                    .flatten()
                    .filter(|e| read_marker(&e.path()).is_some())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                all.sort();
                all.dedup();
                all
            } else {
                for r in &requested {
                    if !catalog.contains_key(r) && !fdt_installed(r) && installed(r).is_empty() {
                        return Err(crate::failure::Failure::new(Kind::NotFound, format!("No hay ninguna skill «{r}».")).hint("fdt skills list").into());
                    }
                }
                requested.clone()
            };
            let mut done = Vec::new();
            for name in &names {
                for dir in &dirs {
                    let target = dir.join(name);
                    if target.exists() {
                        if read_marker(&target).is_none() {
                            return Err(fail(Kind::Error, format!("{} no lo instaló fdt: no se borra.", target.display())));
                        }
                        std::fs::remove_dir_all(&target)?;
                        done.push(json!({"skill": name, "path": target}));
                    }
                }
            }
            if done.is_empty() && !requested.iter().any(|r| r == "all") {
                return Err(crate::failure::Failure::new(
                    Kind::NotFound,
                    format!("{} no está instalada en {}.", names.join(", "), dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(" ni en ")),
                )
                .hint("fdt skills list (y -g para las de tu usuario)")
                .into());
            }
            // No empty .claude/skills or .agents/skills (nor their parent) left behind.
            for dir in &dirs {
                remove_if_empty(dir);
                if let Some(parent) = dir.parent()
                    && parent != base
                {
                    remove_if_empty(parent);
                }
            }
            ("quitadas", done)
        }
    };
    crate::ui::success(&format!("Skills {verb}: {}.", done.len()));
    ctx.out.print(&json!({"ok": true, "changed": done}), || {
        done.iter().map(|d| format!("  {}", crate::output::s(d, "path"))).collect::<Vec<_>>().join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_basics_skill() {
        let catalog = embedded_catalog();
        let basics = catalog.get("fdt-basics").expect("skills/fdt-basics must be embedded");
        assert!(basics.contains_key("SKILL.md"));
        assert!(description(basics).is_some());
    }

    #[test]
    fn installs_and_refuses_hand_written_skills() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        files.insert("SKILL.md".to_owned(), b"---\nname: x\n---\n".to_vec());
        install(dir.path(), "x", &files, "test").unwrap();
        assert!(read_marker(&dir.path().join("x")).is_some());
        install(dir.path(), "x", &files, "test").unwrap();
        std::fs::create_dir_all(dir.path().join("mine")).unwrap();
        assert!(install(dir.path(), "mine", &files, "test").is_err());
    }
}
