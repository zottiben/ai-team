//! Compile the frontend into the binary.
//!
//! `ui/dist` is committed, so `cargo install` works on a machine with a Rust toolchain
//! and nothing else. Node is therefore a build *convenience*, never a build
//! requirement: this script rebuilds the bundle when npm is present and a source file
//! is newer than the committed output, and otherwise embeds what is already there.
//!
//! It never fails the build over the frontend. Someone changing the store should not be
//! stopped by npm. CI is where a stale or broken bundle is caught, with npm's own error
//! message and a `git diff --exit-code` on `ui/dist`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    let ui = PathBuf::from(env("CARGO_MANIFEST_DIR"))
        .join("../../ui")
        .canonicalize()
        .expect("the ui directory is part of the repo");
    let dist = ui.join("dist");

    for path in [
        "src",
        "index.html",
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "tsconfig.json",
    ] {
        println!("cargo:rerun-if-changed={}", ui.join(path).display());
    }
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-env-changed=AI_TEAM_SKIP_UI_BUILD");

    if should_build(&ui, &dist) {
        build(&ui);
    }

    let manifest = match collect(&dist) {
        Ok(files) if !files.is_empty() => files,
        _ => {
            // A clear message at build time beats a blank page at run time.
            println!(
                "cargo:warning=no frontend bundle at {} - `ait ui` will serve a \
                 placeholder. Run `npm ci && npm run build` in ui/ to fix it.",
                dist.display()
            );
            Vec::new()
        }
    };

    let out = PathBuf::from(env("OUT_DIR")).join("assets.rs");
    std::fs::write(&out, render(&manifest)).expect("writing the asset table");
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("cargo sets {key}"))
}

fn should_build(ui: &Path, dist: &Path) -> bool {
    if std::env::var_os("AI_TEAM_SKIP_UI_BUILD").is_some() {
        return false;
    }
    if !has_npm() {
        return false;
    }
    match modified(&dist.join("index.html")) {
        // No bundle at all: build it if we can.
        None => true,
        // Rebuild only when a source is genuinely newer. Otherwise every `cargo build`
        // pays for an npm run that changes nothing.
        Some(built) => newest_source(ui).is_some_and(|source| source > built),
    }
}

fn has_npm() -> bool {
    Command::new("npm")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn build(ui: &Path) {
    if !ui.join("node_modules").exists() {
        run(ui, &["ci", "--no-audit", "--no-fund"])
            // A lockfile that has drifted makes `npm ci` refuse; `install` is the
            // honest fallback rather than a reason to give up on the bundle.
            .or_else(|| run(ui, &["install", "--no-audit", "--no-fund"]));
    }
    if run(ui, &["run", "build"]).is_none() {
        println!("cargo:warning=the frontend build failed - embedding the previous bundle");
    }
}

fn run(dir: &Path, args: &[&str]) -> Option<()> {
    let status = Command::new("npm")
        .args(args)
        .current_dir(dir)
        .status()
        .ok()?;
    status.success().then_some(())
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn newest_source(ui: &Path) -> Option<SystemTime> {
    let mut newest = None;
    for file in [
        "index.html",
        "package.json",
        "vite.config.ts",
        "tsconfig.json",
    ] {
        newest = newest.max(modified(&ui.join(file)));
    }
    let mut stack = vec![ui.join("src")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                newest = newest.max(modified(&path));
            }
        }
    }
    newest
}

/// Every file under `dist`, as (url path, mime, absolute source path).
fn collect(dist: &Path) -> std::io::Result<Vec<(String, &'static str, PathBuf)>> {
    let mut files = Vec::new();
    let mut stack = vec![dist.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(dist)
                .expect("walked from dist")
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, mime(&path), path));
        }
    }
    // Sorted so the generated file is stable, and a rebuild that changes nothing
    // produces no diff.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

fn render(files: &[(String, &'static str, PathBuf)]) -> String {
    use std::fmt::Write as _;

    let mut out = String::from(
        "// Generated by build.rs. The bundle, compiled in.\n\
         pub(crate) static ASSETS: &[(&str, &str, &[u8])] = &[\n",
    );
    for (path, mime, source) in files {
        let _ = writeln!(
            out,
            "    ({:?}, {:?}, include_bytes!({:?})),",
            path,
            mime,
            source.display().to_string()
        );
    }
    out.push_str("];\n");
    out
}
