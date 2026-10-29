use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{error::Result, paths, state::AppState};

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static PROCESS_START_EPOCH_MS: OnceLock<u64> = OnceLock::new();

/// Record the instant the process started. Call as the first line of `main`.
pub fn init_process_start() {
    let _ = PROCESS_START.set(Instant::now());
    let _ = PROCESS_START_EPOCH_MS.set(now_epoch_ms());
}

pub fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

/// Milliseconds elapsed since the process started.
pub fn since_start_ms() -> f64 {
    process_start().elapsed().as_secs_f64() * 1000.0
}

pub fn process_start_epoch_ms() -> u64 {
    *PROCESS_START_EPOCH_MS.get_or_init(now_epoch_ms)
}

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A named checkpoint relative to process start.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Mark {
    pub name: String,
    pub at_ms: f64,
}

/// Collects startup phases. Cheap; each `mark` is a single clock read.
#[derive(Default)]
pub struct Timeline {
    marks: Vec<Mark>,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark(&mut self, name: impl Into<String>) -> &mut Self {
        self.marks.push(Mark {
            name: name.into(),
            at_ms: since_start_ms(),
        });
        self
    }

    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    pub fn record(&self) {
        let logs = paths::logs_dir();
        for mark in &self.marks {
            crate::logging::record_timing(&logs, &mark.name, mark.at_ms, Some("startup"));
        }
    }
}

struct Sample {
    name: &'static str,
    ms: f64,
    note: String,
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_secs_f64() * 1000.0)
}

/// Headless benchmark harness (`kestrel bench`). Everything measured here is
/// on the hot path of the launcher; numbers land in the terminal and in
/// `<logs>/timings.jsonl`.
pub fn run(json: bool) -> Result<()> {
    let mut samples: Vec<Sample> = Vec::new();

    // 1. Cached state load (this is the one thing on the instant-paint path).
    let (state, ms) = time(AppState::load_or_default);
    let note = format!(
        "{} profile(s), {} account(s)",
        state.profiles.len(),
        state.accounts.len()
    );
    samples.push(Sample {
        name: "cache_load",
        ms,
        note,
    });

    // 2. State serialisation + atomic write.
    let (_, ms) = time(|| {
        let _ = state.save();
    });
    samples.push(Sample {
        name: "state_save",
        ms,
        note: format!("{} bytes", std::fs::metadata(paths::state_file()).map(|m| m.len()).unwrap_or(0)),
    });

    // 3. SHA-1 throughput over a 32 MiB buffer (asset verification path).
    let data = vec![0x5au8; 32 * 1024 * 1024];
    let (_, ms) = time(|| {
        use sha1::Digest;
        let mut hasher = sha1::Sha1::new();
        hasher.update(&data);
        hasher.finalize()
    });
    samples.push(Sample {
        name: "sha1_32mib",
        ms,
        note: format!("{:.0} MiB/s", 32.0 / (ms / 1000.0).max(0.000_001)),
    });

    // 4. JSON parse throughput on a synthetic version-manifest-sized payload.
    let payload = synthetic_manifest(2_000);
    let (_, ms) = time(|| serde_json::from_str::<serde_json::Value>(&payload));
    samples.push(Sample {
        name: "json_parse_2k",
        ms,
        note: format!("{} KiB", payload.len() / 1024),
    });

    // 5. Network: Mojang version manifest, with the pooled HTTP/2 client.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| crate::error::Error::msg(format!("tokio runtime: {e}")))?;
    let (net, ms) = time(|| {
        rt.block_on(async {
            let start = Instant::now();
            let res = crate::http::get_bytes(crate::http::PISTON_MANIFEST_URL).await;
            let bytes = res.map(|b| b.len()).unwrap_or(0);
            (bytes, start.elapsed().as_secs_f64() * 1000.0)
        })
    });
    samples.push(Sample {
        name: "net_manifest",
        ms,
        note: format!("{} KiB payload", net.0 / 1024),
    });

    for sample in &samples {
        tracing::info!(
            target: "bench",
            "{:<16} {:>8.2} ms  {}",
            sample.name,
            sample.ms,
            sample.note
        );
        crate::logging::record_timing(&paths::logs_dir(), sample.name, sample.ms, Some(&sample.note));
    }

    if json {
        let out = serde_json::json!({
            "process_start_epoch_ms": process_start_epoch_ms(),
            "samples": samples.iter().map(|s| serde_json::json!({
                "name": s.name, "ms": s.ms, "note": s.note
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        println!("Kestrel bench (cold, single run)\n");
        println!("{:<16} {:>10}  notes", "phase", "time");
        println!("{}", "-".repeat(58));
        for s in &samples {
            println!("{:<16} {:>7.2} ms  {}", s.name, s.ms, s.note);
        }
        println!("\nsamples appended to {}", paths::logs_dir().join("timings.jsonl").display());
    }

    Ok(())
}

fn synthetic_manifest(versions: usize) -> String {
    let versions: Vec<serde_json::Value> = (0..versions)
        .map(|i| {
            serde_json::json!({
                "id": format!("1.{}.{}", i / 100, i % 100),
                "type": if i % 5 == 0 { "snapshot" } else { "release" },
                "url": "https://piston-meta.mojang.com/v1/packages/deadbeef/version.json",
                "time": "2026-01-01T00:00:00+00:00",
                "releaseTime": "2026-01-01T00:00:00+00:00",
                "sha1": "0000000000000000000000000000000000000000",
                "complianceLevel": 1,
            })
        })
        .collect();
    serde_json::json!({
        "latest": { "release": "1.21.5", "snapshot": "26w01a" },
        "versions": versions,
    })
    .to_string()
}

pub fn duration_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
