//! Event sourcing — structured JSONL event log for all server activities.
//!
//! Every server event (file change, reload, WS connect, JS error, etc.) is logged
//! as a single JSON line to `.hotplate/events-{session}.jsonl`.
//!
//! This provides AI agents with rich context to diagnose UI issues:
//!   - What files changed and when
//!   - What errors occurred in the browser (console, JS, network)
//!   - HTTP request timeline
//!   - WebSocket connection lifecycle

use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

// ───────────────────── Event types ─────────────────────

/// All event kinds that Hotplate can produce.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data")]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum EventData {
    /// Server started with given configuration.
    ServerStart {
        port: u16,
        host: String,
        root: String,
        https: bool,
        live_reload: bool,
    },

    /// Server stopped.
    ServerStop {
        uptime_secs: u64,
    },

    /// A file was changed on disk (create/modify/remove).
    FileChange {
        path: String,
        ext: String,
        change: String, // "create" | "modify" | "remove"
    },

    /// A reload event was broadcast to connected browsers.
    ReloadTrigger {
        path: String,
        reload_type: String, // "full" | "css"
    },

    /// A browser connected via WebSocket.
    WsConnect {
        client_id: String,
        url: String,
        user_agent: String,
        viewport: (u32, u32),
    },

    /// A browser disconnected.
    WsDisconnect {
        client_id: String,
    },

    /// An HTTP request was handled.
    HttpRequest {
        method: String,
        path: String,
        status: u16,
        duration_ms: u64,
    },

    /// A JavaScript error occurred in the browser.
    JsError {
        message: String,
        source: String,
        line: u32,
        col: u32,
        stack: String,
    },

    /// A console message from the browser.
    ConsoleLog {
        level: String, // "log" | "warn" | "error" | "info"
        message: String,
    },

    /// A network error from the browser (failed fetch, non-ok status).
    NetworkError {
        url: String,
        method: String,
        status: u16,
        error: String,
    },
}

/// A single event with timestamp and session ID.
#[derive(Debug, Clone, Serialize)]
pub struct HotplateEvent {
    /// ISO 8601 timestamp (e.g. "2026-02-09T14:30:01.123Z")
    pub ts: String,
    /// Session ID (shared across one server run)
    pub session: String,
    /// Event payload
    #[serde(flatten)]
    pub data: EventData,
}

// ───────────────────── Session ID ─────────────────────

/// Generate a short session ID from current timestamp (e.g. "20260209-143001").
pub fn generate_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Convert to readable format: YYYYMMDD-HHMMSS
    let secs_per_day = 86400u64;
    let days_since_epoch = now / secs_per_day;
    let time_of_day = now % secs_per_day;

    // Simple date calculation (good enough for session IDs)
    let (year, month, day) = days_to_ymd(days_since_epoch);
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Get current ISO 8601 timestamp string.
fn now_iso() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();

    let secs_per_day = 86400u64;
    let days = secs / secs_per_day;
    let time_of_day = secs % secs_per_day;

    let (year, month, day) = days_to_ymd(days);
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hours, minutes, seconds, millis
    )
}

// ───────────────────── EventLogger ─────────────────────

/// Async event logger — receives events via mpsc channel, writes JSONL to disk.
#[derive(Clone)]
pub struct EventLogger {
    tx: mpsc::UnboundedSender<HotplateEvent>,
    session: String,
}

impl EventLogger {
    /// Create a new EventLogger that writes to `.hotplate/events-{session}.jsonl`
    /// in the given workspace directory.
    ///
    /// Spawns a background tokio task for non-blocking writes.
    pub fn new(workspace: &Path) -> Self {
        let session = generate_session_id();
        let (tx, rx) = mpsc::unbounded_channel();

        let log_dir = workspace.join(".hotplate");
        let log_file = log_dir.join(format!("events-{}.jsonl", session));

        // Spawn writer task
        let session_clone = session.clone();
        tokio::spawn(async move {
            Self::writer_loop(rx, log_dir, log_file, &session_clone).await;
        });

        EventLogger { tx, session }
    }

    /// Create a no-op logger that discards all events (when --no-event-log).
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        EventLogger {
            tx,
            session: "noop".into(),
        }
    }

    /// Background writer loop — creates dir, opens file, writes events.
    async fn writer_loop(
        mut rx: mpsc::UnboundedReceiver<HotplateEvent>,
        log_dir: PathBuf,
        log_file: PathBuf,
        session: &str,
    ) {
        use std::io::Write;

        // Create .hotplate directory
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("  ⚠ Failed to create event log dir: {}", e);
            return;
        }

        // Clean up old session files (keep last 10)
        Self::cleanup_old_sessions(&log_dir, 10);

        // Open log file (append mode)
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  ⚠ Failed to open event log: {}", e);
                return;
            }
        };

        println!("  📋 Events:  .hotplate/events-{}.jsonl", session);

        // Write events as they arrive
        while let Some(event) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = writeln!(file, "{}", json);
                let _ = file.flush();
            }
        }
    }

    /// Remove old session files, keeping only the most recent `keep` files.
    fn cleanup_old_sessions(log_dir: &Path, keep: usize) {
        let mut files: Vec<_> = std::fs::read_dir(log_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("events-")
                    && e.file_name().to_string_lossy().ends_with(".jsonl")
            })
            .collect();

        if files.len() <= keep {
            return;
        }

        // Sort by name (which includes timestamp, so chronological)
        files.sort_by_key(|e| e.file_name());

        // Remove oldest files
        let to_remove = files.len() - keep;
        for entry in files.into_iter().take(to_remove) {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    /// Log an event (non-blocking — sends to writer task via channel).
    pub fn log(&self, data: EventData) {
        let event = HotplateEvent {
            ts: now_iso(),
            session: self.session.clone(),
            data,
        };
        let _ = self.tx.send(event);
    }

    /// Get the session ID.
    #[allow(dead_code)]
    pub fn session(&self) -> &str {
        &self.session
    }
}
