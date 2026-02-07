//! File system watcher — debounced, filtered, broadcasts reload events.

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Directories/files to always ignore.
const IGNORE_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".venv"];
const IGNORE_EXTS: &[&str] = &["pyc", "pyo", "swp", "swo", "tmp"];

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

fn should_ignore(paths: &[PathBuf], root: &PathBuf, user_globs: &Option<GlobSet>) -> bool {
    paths.iter().all(|p| {
        let s = p.to_string_lossy();
        // Ignored directories
        if IGNORE_DIRS.iter().any(|d| s.contains(d)) {
            return true;
        }
        // Ignored extensions
        if let Some(ext) = p.extension() {
            if IGNORE_EXTS.contains(&ext.to_string_lossy().as_ref()) {
                return true;
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
pub fn spawn(
    root: PathBuf,
    reload_tx: broadcast::Sender<String>,
    ignore_patterns: &[String],
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
                if should_ignore(&event.paths, &watch_root, &user_globs) {
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
