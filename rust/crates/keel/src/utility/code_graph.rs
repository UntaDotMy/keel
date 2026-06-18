//! Purpose: Deterministic codebase-understanding graph (audit finding A1).
//!   Extracts a committable structural graph — per-file symbol definitions and
//!   import edges, plus resolved cross-file dependency edges — without an LLM and
//!   without a tree-sitter grammar dependency. Backs `code-graph build` (write the
//!   artifact) and `code-graph impact` (reverse-dependency closure for changed
//!   files). This is the structural layer the flat SYSTEM_MAP and the manual
//!   `preserve-existing-flow` owner trace lacked: the same code always yields the
//!   same edges, so the artifact is reproducible and reviewable.
//! Caller: commands.rs `code-graph` dispatch arm.
//! Dependencies: std::fs/path, crate::args::FlagSet, crate::runtime path helpers,
//!   serde_json (already a workspace dependency).
//! Main Functions: run_code_graph_command, build_graph, impact_of.
//! Side Effects: Reads workspace files; `build` writes the JSON artifact under the
//!   workspace (default `.understand/code-graph.json`, committable).
//!
//! Determinism is the contract. Extraction is line-based (no network, no model,
//! no global state): nodes are sorted by path, edges by (from,to,kind), and every
//! per-file list is sorted, so two runs over the same tree produce byte-identical
//! JSON. Cross-file edges are only emitted when an import resolves to a real file
//! on disk (relative JS/TS/Python imports and Rust `mod` declarations); unresolved
//! imports (bare module paths, external packages) are kept as node `imports`
//! strings but never invented as edges. This keeps the impact closure honest.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::runtime::{display_path, resolve_repository_root};

/// Schema version of the emitted artifact. Bump when the JSON shape changes so a
/// reader can refuse an incompatible graph rather than misparse it.
const GRAPH_VERSION: u64 = 1;

/// Default committable artifact location, relative to the workspace root. Mirrors
/// the "graph-as-code" idea: teammates and later sessions read the artifact
/// instead of re-running the scan.
const DEFAULT_ARTIFACT: &str = ".understand/code-graph.json";

/// Upper bound on files scanned, matching `code-search`'s ceiling so a pathological
/// tree cannot make the command run unbounded.
const MAX_FILES: usize = 10_000;

/// Cap on definitions recorded per file so one generated/vendored megafile cannot
/// bloat the artifact. The structural signal is in the first symbols anyway.
const MAX_DEFS_PER_FILE: usize = 200;

/// A single source file in the graph.
struct Node {
    /// Workspace-relative, forward-slashed path. Stable id across platforms.
    id: String,
    lang: &'static str,
    defines: Vec<String>,
    imports: Vec<String>,
}

/// One dependency edge between two in-repo files.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
struct Edge {
    from: String,
    to: String,
    kind: &'static str,
}

/// The whole graph, ready to serialize.
pub struct CodeGraph {
    root: PathBuf,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

/// CLI: `keel code-graph [build|impact] [flags]`.
pub fn run_code_graph_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: keel code-graph <build|impact> [flags]\n\
             \n\
             build    Scan the workspace and write a deterministic structural graph (JSON).\n\
             impact   Report the files that depend (transitively) on the changed files.\n\
             \n\
             Flags:\n\
             \x20 --workspace-root <path>  Root to scan (default: resolved repository root).\n\
             \x20 --output <path>          Artifact path for build (default: {DEFAULT_ARTIFACT}).\n\
             \x20 --changed <a,b,c>        Comma-separated changed files for impact.\n\
             \x20 --json                   Machine-readable output."
        );
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "build" => run_build(&arguments[1..], standard_output, standard_error),
        "impact" => run_impact(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "code-graph: unknown subcommand: {other}");
            1
        }
    }
}

fn resolve_root(
    workspace_root: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<PathBuf> {
    let root = if workspace_root.is_empty() {
        match resolve_repository_root("") {
            Ok(path) => path,
            Err(_) => {
                let _ = writeln!(standard_error, "{label}: no repository root found");
                return None;
            }
        }
    } else {
        PathBuf::from(workspace_root)
    };
    if !root.is_dir() {
        let _ = writeln!(
            standard_error,
            "{label}: workspace-root not a directory: {}",
            display_path(&root)
        );
        return None;
    }
    Some(root)
}

fn run_build(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("code-graph build");
    flags.string_flag("workspace-root", "");
    flags.string_flag("output", DEFAULT_ARTIFACT);
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "code-graph build: {}", error.message);
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("workspace-root"),
        "code-graph build",
        standard_error,
    ) else {
        return 1;
    };

    let graph = build_graph(&root);
    let artifact = graph.to_json();
    let serialized = match serde_json::to_string_pretty(&artifact) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(standard_error, "code-graph build: serialize: {error}");
            return 1;
        }
    };

    let output_relative = flags.string_value("output");
    let output_path = root.join(output_relative);
    if let Some(parent) = output_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            let _ = writeln!(
                standard_error,
                "code-graph build: create {}: {error}",
                display_path(parent)
            );
            return 1;
        }
    }
    if let Err(error) = fs::write(&output_path, format!("{serialized}\n")) {
        let _ = writeln!(
            standard_error,
            "code-graph build: write {}: {error}",
            display_path(&output_path)
        );
        return 1;
    }

    if flags.bool_value("json") {
        let payload = serde_json::json!({
            "built": true,
            "artifact": display_path(&output_path),
            "fileCount": graph.nodes.len(),
            "edgeCount": graph.edges.len(),
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "code-graph build: render json: {error}");
                1
            }
        }
    } else {
        let _ = writeln!(
            standard_output,
            "code-graph build: {} file(s), {} internal edge(s) -> {}",
            graph.nodes.len(),
            graph.edges.len(),
            display_path(&output_path)
        );
        0
    }
}

fn run_impact(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("code-graph impact");
    flags.string_flag("workspace-root", "");
    flags.string_flag("changed", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "code-graph impact: {}", error.message);
        return 1;
    }
    let changed_raw = flags.string_value("changed");
    if changed_raw.trim().is_empty() {
        let _ = writeln!(
            standard_error,
            "code-graph impact: --changed required (example: --changed src/a.rs,src/b.rs)"
        );
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("workspace-root"),
        "code-graph impact",
        standard_error,
    ) else {
        return 1;
    };

    // Build the graph fresh so impact never reports against a stale artifact.
    let graph = build_graph(&root);
    let changed: Vec<String> = changed_raw
        .split(',')
        .map(|item| normalize_relative(item.trim()))
        .filter(|item| !item.is_empty())
        .collect();
    let impacted = graph.impact_of(&changed);

    if flags.bool_value("json") {
        let payload = serde_json::json!({
            "changed": changed,
            "impacted": impacted,
            "impactedCount": impacted.len(),
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "code-graph impact: render json: {error}");
                1
            }
        }
    } else if impacted.is_empty() {
        let _ = writeln!(
            standard_output,
            "code-graph impact: no in-repo files depend on the changed file(s)"
        );
        0
    } else {
        let _ = writeln!(
            standard_output,
            "code-graph impact: {} file(s) depend on the changed file(s):",
            impacted.len()
        );
        for path in &impacted {
            let _ = writeln!(standard_output, "  {path}");
        }
        0
    }
}

/// Scan `root` and produce the deterministic graph. Public for unit tests and the
/// learn loop's potential reuse.
pub fn build_graph(root: &Path) -> CodeGraph {
    let mut files = Vec::new();
    collect_source_files(root, &mut files);
    files.sort();

    // Map every relative path to its node index for edge resolution.
    let mut path_set: BTreeSet<String> = BTreeSet::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut raw_imports: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for absolute in &files {
        let relative = match absolute.strip_prefix(root) {
            Ok(stripped) => normalize_relative(&display_path(stripped)),
            Err(_) => continue,
        };
        let Some(lang) = language_for(absolute) else {
            continue;
        };
        let text = match fs::read_to_string(absolute) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let mut defines = extract_definitions(lang, &text);
        defines.sort();
        defines.dedup();
        defines.truncate(MAX_DEFS_PER_FILE);

        let mut imports = extract_imports(lang, &text);
        imports.sort();
        imports.dedup();

        path_set.insert(relative.clone());
        raw_imports.insert(relative.clone(), imports.clone());
        nodes.push(Node {
            id: relative,
            lang,
            defines,
            imports,
        });
    }

    // Resolve imports to in-repo files -> internal edges. Only edges whose target
    // exists in path_set are emitted; everything else stays a node `imports` entry.
    let mut edges: BTreeSet<Edge> = BTreeSet::new();
    for node in &nodes {
        let lang = node.lang;
        if let Some(specs) = raw_imports.get(&node.id) {
            for spec in specs {
                if let Some(target) = resolve_import(lang, &node.id, spec, &path_set) {
                    if target != node.id {
                        edges.insert(Edge {
                            from: node.id.clone(),
                            to: target,
                            kind: "imports",
                        });
                    }
                }
            }
        }
    }

    let mut edges: Vec<Edge> = edges.into_iter().collect();
    edges.sort();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    CodeGraph {
        root: root.to_path_buf(),
        nodes,
        edges,
    }
}

impl CodeGraph {
    /// Reverse-dependency closure: every in-repo file that transitively imports
    /// any of `changed`. The changed files themselves are excluded from the result
    /// (the caller already knows those changed). Deterministic, sorted output.
    pub fn impact_of(&self, changed: &[String]) -> Vec<String> {
        // Reverse adjacency: target -> importers.
        let mut importers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &self.edges {
            importers
                .entry(edge.to.as_str())
                .or_default()
                .push(edge.from.as_str());
        }
        let changed_set: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
        let mut impacted: BTreeSet<String> = BTreeSet::new();
        let mut stack: Vec<String> = changed.to_vec();
        let mut seen: BTreeSet<String> = changed_set.iter().map(|s| s.to_string()).collect();
        while let Some(current) = stack.pop() {
            if let Some(dependents) = importers.get(current.as_str()) {
                for dependent in dependents {
                    let dependent = dependent.to_string();
                    if seen.insert(dependent.clone()) {
                        impacted.insert(dependent.clone());
                        stack.push(dependent);
                    }
                }
            }
        }
        // Never report a changed file as impacted by itself.
        for change in changed {
            impacted.remove(change);
        }
        impacted.into_iter().collect()
    }

    fn to_json(&self) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = self
            .nodes
            .iter()
            .map(|node| {
                serde_json::json!({
                    "id": node.id,
                    "lang": node.lang,
                    "defines": node.defines,
                    "imports": node.imports,
                })
            })
            .collect();
        let edges: Vec<serde_json::Value> = self
            .edges
            .iter()
            .map(|edge| {
                serde_json::json!({
                    "from": edge.from,
                    "to": edge.to,
                    "kind": edge.kind,
                })
            })
            .collect();
        serde_json::json!({
            "version": GRAPH_VERSION,
            "generator": "keel-code-graph",
            "root": display_path(&self.root),
            "fileCount": self.nodes.len(),
            "edgeCount": self.edges.len(),
            "nodes": nodes,
            "edges": edges,
        })
    }
}

/// Forward-slash and trim a relative path to a stable cross-platform id.
fn normalize_relative(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_string()
}

fn language_for(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("rs") => Some("rust"),
        Some("js" | "jsx" | "mjs" | "cjs") => Some("javascript"),
        Some("ts" | "tsx") => Some("typescript"),
        Some("py") => Some("python"),
        Some("go") => Some("go"),
        _ => None,
    }
}

/// Extract top-level symbol definitions deterministically by line prefix. This is
/// intentionally shallow (no parser): it captures the names a reviewer scans for,
/// stable across runs, and never executes or interprets the code.
fn extract_definitions(lang: &str, text: &str) -> Vec<String> {
    let mut defs = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        match lang {
            "rust" => {
                capture_after(
                    line,
                    &["pub fn ", "fn ", "pub async fn ", "async fn "],
                    &mut defs,
                    "fn ",
                );
                capture_after(line, &["pub struct ", "struct "], &mut defs, "struct ");
                capture_after(line, &["pub enum ", "enum "], &mut defs, "enum ");
                capture_after(line, &["pub trait ", "trait "], &mut defs, "trait ");
            }
            "javascript" | "typescript" => {
                capture_after(
                    line,
                    &[
                        "export function ",
                        "function ",
                        "export async function ",
                        "async function ",
                    ],
                    &mut defs,
                    "fn ",
                );
                capture_after(line, &["export class ", "class "], &mut defs, "class ");
                capture_after(line, &["export const ", "export let "], &mut defs, "const ");
            }
            "python" => {
                capture_after(line, &["def ", "async def "], &mut defs, "def ");
                capture_after(line, &["class "], &mut defs, "class ");
            }
            "go" => {
                capture_after(line, &["func "], &mut defs, "func ");
                capture_after(line, &["type "], &mut defs, "type ");
            }
            _ => {}
        }
    }
    defs
}

/// If `line` starts with any prefix in `prefixes`, push `<label><identifier>` to
/// `defs`. The identifier is the first token of the remainder, stripped of common
/// trailing delimiters so `fn foo(` and `struct Bar {` yield `foo` / `Bar`.
fn capture_after(line: &str, prefixes: &[&str], defs: &mut Vec<String>, label: &str) {
    for prefix in prefixes {
        if let Some(rest) = line.strip_prefix(prefix) {
            let ident = first_identifier(rest);
            if !ident.is_empty() {
                defs.push(format!("{label}{ident}"));
            }
            return;
        }
    }
}

/// First identifier token: letters/digits/underscore, stopping at any other char.
fn first_identifier(text: &str) -> String {
    text.trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Extract raw import specifier strings deterministically per language. Returns the
/// module/path tokens; resolution to in-repo files happens separately.
fn extract_imports(lang: &str, text: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        match lang {
            "rust" => {
                // `mod foo;` is the resolvable file edge; `use ...` is recorded as
                // a reference string (crate-relative resolution is out of scope).
                if let Some(rest) = line.strip_prefix("mod ") {
                    let ident = first_identifier(rest);
                    if !ident.is_empty() {
                        imports.push(format!("mod:{ident}"));
                    }
                } else if let Some(rest) = line.strip_prefix("pub mod ") {
                    let ident = first_identifier(rest);
                    if !ident.is_empty() {
                        imports.push(format!("mod:{ident}"));
                    }
                } else if let Some(rest) = line.strip_prefix("use ") {
                    let path = rest
                        .trim_end_matches(';')
                        .split(" as ")
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !path.is_empty() {
                        imports.push(path.to_string());
                    }
                }
            }
            "javascript" | "typescript" => {
                if let Some(spec) = js_import_spec(line) {
                    imports.push(spec);
                }
            }
            "python" => {
                if let Some(rest) = line.strip_prefix("from ") {
                    let module = rest.split(" import ").next().unwrap_or("").trim();
                    if !module.is_empty() {
                        imports.push(module.to_string());
                    }
                } else if let Some(rest) = line.strip_prefix("import ") {
                    let module = rest.split([' ', ',']).next().unwrap_or("").trim();
                    if !module.is_empty() {
                        imports.push(module.to_string());
                    }
                }
            }
            "go" => {
                // Single-line `import "path"` only; import blocks are recorded by
                // their quoted entries on their own lines.
                if let Some(spec) = go_import_spec(line) {
                    imports.push(spec);
                }
            }
            _ => {}
        }
    }
    imports
}

/// Pull the quoted module specifier from a JS/TS import/require/export-from line.
fn js_import_spec(line: &str) -> Option<String> {
    let is_import =
        line.starts_with("import ") || line.starts_with("export ") || line.contains("require(");
    if !is_import {
        return None;
    }
    // Take the first single- or double-quoted token.
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        if quote == b'\'' || quote == b'"' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != quote {
                end += 1;
            }
            if end <= bytes.len() {
                return Some(line[start..end].to_string());
            }
        }
        index += 1;
    }
    None
}

/// Pull the quoted path from a single-line Go `import "..."` or a block entry line.
fn go_import_spec(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let candidate = trimmed.strip_prefix("import ").unwrap_or(trimmed).trim();
    let inner = candidate.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

/// Resolve an import specifier to an in-repo file id, or None if it does not map
/// to a file we scanned (external package, unresolved crate path, etc.).
fn resolve_import(
    lang: &str,
    from: &str,
    spec: &str,
    path_set: &BTreeSet<String>,
) -> Option<String> {
    match lang {
        "rust" => {
            let module = spec.strip_prefix("mod:")?;
            // `mod foo;` resolves to a sibling `foo.rs` or `foo/mod.rs`.
            let dir = parent_dir(from);
            for candidate in [
                join_rel(&dir, &format!("{module}.rs")),
                join_rel(&dir, &format!("{module}/mod.rs")),
            ] {
                if path_set.contains(&candidate) {
                    return Some(candidate);
                }
            }
            None
        }
        "javascript" | "typescript" => {
            if !spec.starts_with('.') {
                return None; // bare/package import, not an in-repo file
            }
            let dir = parent_dir(from);
            let base = join_rel(&dir, spec);
            resolve_with_extensions(
                &base,
                &["", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"],
                &["/index.ts", "/index.tsx", "/index.js", "/index.jsx"],
                path_set,
            )
        }
        "python" => {
            // Only resolve explicit relative imports (`.module` / `..pkg.module`).
            if !spec.starts_with('.') {
                return None;
            }
            let leading_dots = spec.chars().take_while(|c| *c == '.').count();
            let remainder = &spec[leading_dots..];
            let mut dir = parent_dir(from);
            // Each extra leading dot beyond the first climbs one package level.
            for _ in 1..leading_dots {
                dir = parent_dir(&dir);
            }
            let rel = remainder.replace('.', "/");
            let base = if rel.is_empty() {
                dir.clone()
            } else {
                join_rel(&dir, &rel)
            };
            resolve_with_extensions(&base, &[".py"], &["/__init__.py"], path_set)
        }
        // Go package paths do not map to single files without the module root; we
        // intentionally do not guess, keeping the impact closure honest.
        _ => None,
    }
}

/// Try `base + ext` for each extension, then `base + index` for each index suffix.
fn resolve_with_extensions(
    base: &str,
    extensions: &[&str],
    index_suffixes: &[&str],
    path_set: &BTreeSet<String>,
) -> Option<String> {
    for extension in extensions {
        let candidate = normalize_relative(&format!("{base}{extension}"));
        if !candidate.is_empty() && path_set.contains(&candidate) {
            return Some(candidate);
        }
    }
    for suffix in index_suffixes {
        let candidate = normalize_relative(&format!("{base}{suffix}"));
        if path_set.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Parent directory of a forward-slashed relative path ("" for a root-level file).
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..index].to_string(),
        None => String::new(),
    }
}

/// Join a relative path against a base dir and collapse `.`/`..` segments.
fn join_rel(dir: &str, rel: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    if !dir.is_empty() {
        segments.extend(dir.split('/').filter(|s| !s.is_empty()));
    }
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

fn collect_source_files(root: &Path, files: &mut Vec<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        if files.len() >= MAX_FILES {
            return;
        }
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry_result in entries.flatten() {
            let path = entry_result.path();
            let name = entry_result.file_name().to_string_lossy().to_string();
            if should_skip_entry(&name, &path) {
                continue;
            }
            // Use the dir entry's file type (does NOT follow symlinks) and skip any
            // symlink so a link outside the workspace cannot leak external files.
            let Ok(file_type) = entry_result.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && language_for(&path).is_some() {
                files.push(path);
            }
        }
    }
}

fn should_skip_entry(name: &str, path: &Path) -> bool {
    if name.starts_with('.') {
        return true;
    }
    if path.is_dir() {
        matches!(
            name,
            "node_modules"
                | ".venv"
                | "venv"
                | "env"
                | "vendor"
                | "target"
                | ".gradle"
                | "bin"
                | "obj"
                | "pkg"
                | ".git"
                | "__pycache__"
                | "dist"
                | "build"
                | "tmp"
                | "coverage"
        )
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tempdir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("code-graph-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    #[test]
    fn extracts_rust_defs_and_mod_edges() {
        let root = tempdir("rust");
        write(
            &root,
            "src/main.rs",
            "mod helper;\npub fn main() {}\nstruct Config {}\n",
        );
        write(&root, "src/helper.rs", "pub fn assist() {}\n");

        let graph = build_graph(&root);
        let main = graph
            .nodes
            .iter()
            .find(|n| n.id == "src/main.rs")
            .expect("main node");
        assert!(main.defines.contains(&"fn main".to_string()));
        assert!(main.defines.contains(&"struct Config".to_string()));
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.from == "src/main.rs" && e.to == "src/helper.rs"),
            "mod helper; should resolve to a sibling-file edge"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolves_relative_js_imports_to_files() {
        let root = tempdir("js");
        write(
            &root,
            "app/index.ts",
            "import { x } from './util';\nexport class App {}\n",
        );
        write(&root, "app/util.ts", "export const x = 1;\n");

        let graph = build_graph(&root);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.from == "app/index.ts" && e.to == "app/util.ts"),
            "relative import './util' should resolve to util.ts"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn does_not_invent_edges_for_external_imports() {
        let root = tempdir("external");
        write(
            &root,
            "a.ts",
            "import React from 'react';\nimport { y } from './b';\n",
        );
        write(&root, "b.ts", "export const y = 2;\n");

        let graph = build_graph(&root);
        // 'react' is external -> kept as an imports string, no edge.
        let a = graph.nodes.iter().find(|n| n.id == "a.ts").unwrap();
        assert!(a.imports.contains(&"react".to_string()));
        assert_eq!(graph.edges.len(), 1, "only the in-repo ./b edge is emitted");
        assert_eq!(graph.edges[0].to, "b.ts");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn impact_closure_is_transitive_and_excludes_changed() {
        let root = tempdir("impact");
        // c <- b <- a  (a imports b, b imports c). Changing c impacts b and a.
        write(&root, "c.ts", "export const c = 1;\n");
        write(
            &root,
            "b.ts",
            "import { c } from './c';\nexport const b = c;\n",
        );
        write(
            &root,
            "a.ts",
            "import { b } from './b';\nexport const a = b;\n",
        );

        let graph = build_graph(&root);
        let impacted = graph.impact_of(&["c.ts".to_string()]);
        assert_eq!(impacted, vec!["a.ts".to_string(), "b.ts".to_string()]);
        // The changed file is never reported as impacting itself.
        assert!(!impacted.contains(&"c.ts".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_is_deterministic() {
        let root = tempdir("deterministic");
        write(
            &root,
            "a.py",
            "from .b import thing\ndef run():\n    pass\n",
        );
        write(&root, "b.py", "def thing():\n    pass\n");

        let first = serde_json::to_string(&build_graph(&root).to_json()).unwrap();
        let second = serde_json::to_string(&build_graph(&root).to_json()).unwrap();
        assert_eq!(
            first, second,
            "two builds over the same tree must be identical"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolves_relative_python_import() {
        let root = tempdir("python");
        write(&root, "app/main.py", "from .helper import f\n");
        write(&root, "app/helper.py", "def f():\n    pass\n");

        let graph = build_graph(&root);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.from == "app/main.py" && e.to == "app/helper.py"),
            "from .helper import should resolve within the package"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_pruned_directories() {
        let root = tempdir("pruned");
        write(&root, "src/main.rs", "fn main() {}\n");
        write(&root, "target/junk.rs", "fn junk() {}\n");
        write(&root, "node_modules/dep/index.js", "export const z = 1;\n");

        let graph = build_graph(&root);
        assert!(graph.nodes.iter().any(|n| n.id == "src/main.rs"));
        assert!(
            !graph.nodes.iter().any(|n| n.id.contains("target/")),
            "target/ must be pruned"
        );
        assert!(
            !graph.nodes.iter().any(|n| n.id.contains("node_modules/")),
            "node_modules/ must be pruned"
        );

        let _ = fs::remove_dir_all(root);
    }
}
