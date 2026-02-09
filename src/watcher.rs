//! File system watcher — debounced, filtered, broadcasts reload events.

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Directories/files to always ignore.
const IGNORE_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".venv"];
const IGNORE_EXTS: &[&str] = &["pyc", "pyo", "swp", "swo", "tmp"];

/// Default file extensions to watch (UI-related files).
/// Only files with these extensions trigger a reload.
/// Users can override this with `--watch-ext` or `hotplate.watchExtensions`.
pub const DEFAULT_WATCH_EXTS: &[&str] = &[
    "html", "htm", "css", "scss", "sass", "less",
    "js", "jsx", "ts", "tsx", "mjs", "cjs",
    "json", "svg", "png", "jpg", "jpeg", "gif", "webp", "ico",
    "woff", "woff2", "ttf", "eot",
    "xml", "md", "txt",
];

/// Build a GlobSet from user-provided patterns (e.g. ["**/*.scss", ".vscode/**"]).
fn build_ignore_globs(patterns: &[String]) -> Option<GlobSet> {
    if patterns.is_empty() {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        match Glob::new(pattern) {
            Ok(g) => { builder.add(g); }
            Err(e) => eprintln!("  ⚠ Invalid ignore pattern '{}': {}", pattern, e),
        }
    }
    builder.build().ok()
}

fn should_ignore(
    paths: &[PathBuf],
    root: &PathBuf,
    user_globs: &Option<GlobSet>,
    watch_exts: &Option<HashSet<String>>,
) -> bool {
    paths.iter().all(|p| {
        let s = p.to_string_lossy();
        // Ignored directories
        if IGNORE_DIRS.iter().any(|d| s.contains(d)) {
            return true;
        }
        // Ignored extensions (always blocked)
        if let Some(ext) = p.extension() {
            let ext_lower = ext.to_string_lossy().to_lowercase();
            if IGNORE_EXTS.contains(&ext_lower.as_str()) {
                return true;
            }
        }
        // Watch extension whitelist — only trigger for these extensions
        if let Some(ref exts) = watch_exts {
            match p.extension() {
                Some(ext) => {
                    let ext_lower = ext.to_string_lossy().to_lowercase();
                    if !exts.contains(ext_lower.as_str()) {
                        return true; // not in whitelist → ignore
                    }
                }
                None => return true, // no extension → ignore
            }
        }
        // User-provided glob patterns (matched against relative path)
        if let Some(ref globs) = user_globs {
            let rel = p.strip_prefix(root).unwrap_or(p);
            // Normalize to forward slashes for glob matching (Windows uses backslash)
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if globs.is_match(&rel_str) {
                return true;
            }
        }
        false
    })
}

fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    )
}

/// Spawn a file watcher on a background thread.
/// Sends the relative path of changed files to `reload_tx` (debounced 150ms).
/// `ignore_patterns` are user-provided glob patterns to skip (e.g. "**/*.scss").
/// `watch_extensions` limits which file extensions trigger reloads (e.g. ["html", "css", "js"]).
/// If empty, the default UI-related extensions are used. Pass `["*"]` to watch all files.
pub fn spawn(
    root: PathBuf,
    reload_tx: broadcast::Sender<String>,
    ignore_patterns: &[String],
    watch_extensions: &[String],
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<Event, notify::Error>>();

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        notify::Config::default(),
    )?;

    watcher.watch(&root, RecursiveMode::Recursive)?;

    let user_globs = build_ignore_globs(ignore_patterns);

    // Build watch extensions whitelist
    let watch_exts: Option<HashSet<String>> = {
        let exts: Vec<String> = if watch_extensions.is_empty() {
            // Default: UI-related extensions
            DEFAULT_WATCH_EXTS.iter().map(|s| s.to_string()).collect()
        } else {
            watch_extensions.to_vec()
        };
        // "*" means watch all files (no filter)
        if exts.iter().any(|e| e == "*") {
            None
        } else {
            Some(exts.into_iter().map(|e| e.to_lowercase().trim_start_matches('.').to_string()).collect())
        }
    };

    let watch_root = root.clone();

    // Dedicated OS thread — never blocks tokio
    std::thread::Builder::new()
        .name("fs-watcher".into())
        .spawn(move || {
            let _watcher = watcher; // prevent drop
            let mut last_reload = Instant::now();
            let debounce = Duration::from_millis(150);

            for event in rx {
                let Ok(event) = event else { continue };

                if !is_relevant_event(&event.kind) {
                    continue;
                }
                if should_ignore(&event.paths, &watch_root, &user_globs, &watch_exts) {
                    continue;
                }
                if last_reload.elapsed() < debounce {
                    continue;
                }

                last_reload = Instant::now();

                // Log changed file and send its relative path
                let rel_path = event.paths.first()
                    .map(|p| {
                        let rel = p.strip_prefix(&watch_root).unwrap_or(p);
                        let display = rel.display();
                        println!("  ↻ {}", display);
                        // Normalize to forward slashes
                        rel.to_string_lossy().replace('\\', "/")
                    })
                    .unwrap_or_default();

                let _ = reload_tx.send(rel_path);
            }
        })?;

    Ok(())
}
