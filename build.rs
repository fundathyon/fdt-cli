//! Embeds the agent skills in `skills/<name>/…` so `fdt skills` works offline
//! and always installs the skills that match this binary.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=FDT_VERSION");
    println!("cargo:rerun-if-changed=skills");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("skills");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut code = String::from("pub static EMBEDDED: &[(&str, &str, &[u8])] = &[\n");
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let rel = file.strip_prefix(&root).unwrap();
        let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
        let Some(skill) = parts.next() else { continue };
        let rest: Vec<String> = parts.collect();
        if rest.is_empty() {
            continue; // a loose file in skills/, not part of a skill
        }
        writeln!(
            code,
            "    ({:?}, {:?}, include_bytes!({:?})),",
            skill,
            rest.join("/"),
            file.display().to_string()
        )
        .unwrap();
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("skills.rs");
    std::fs::write(out, code).unwrap();
}
