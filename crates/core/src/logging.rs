use std::path::Path;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Initialise file + stderr logging. Safe to call more than once; later calls
/// are ignored.
pub fn init(logs_dir: &Path) {
    if GUARD.get().is_some() {
        return;
    }
    if let Err(err) = std::fs::create_dir_all(logs_dir) {
        eprintln!("kestrel: could not create log directory {logs_dir:?}: {err}");
        return;
    }

    let file_appender = tracing_appender::rolling::daily(logs_dir, "kestrel.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    if GUARD.set(guard).is_err() {
        return;
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,kestrel_core=debug"));

    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true);
    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
}

/// Append a latency sample to `<logs>/timings.jsonl`. Used by the in-app
/// timing log and by the bench harness.
pub fn record_timing(logs_dir: &Path, name: &str, elapsed_ms: f64, metadata: Option<&str>) {
    use std::io::Write;

    let entry = serde_json::json!({
        "ts": crate::bench::now_epoch_ms(),
        "name": name,
        "ms": elapsed_ms,
        "meta": metadata,
    });
    let line = match serde_json::to_string(&entry) {
        Ok(line) => line,
        Err(_) => return,
    };

    if std::fs::create_dir_all(logs_dir).is_err() {
        return;
    }
    let path = logs_dir.join("timings.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}
