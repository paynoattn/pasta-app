//! Log files (#18): daily rotation in the app log dir, a week kept.
//!
//! Hard rule: clipboard text is never logged, only item ids, sizes and short
//! hashes. `crates/pasta-net/tests/no_content_in_logs.rs` enforces it for the
//! sync path.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Overrides the configured level, e.g. `PASTA_LOG=pasta_net=debug`.
pub const LEVEL_ENV: &str = "PASTA_LOG";
const DEFAULT_LEVEL: &str = "info";
const KEEP_FILES: usize = 7;

/// Starts logging to `dir`. Keep the guard alive until exit so buffered lines get written.
pub fn init(dir: &Path, level: Option<&str>) -> Option<WorkerGuard> {
    let filter = || {
        std::env::var(LEVEL_ENV)
            .ok()
            .and_then(|env| EnvFilter::try_new(env).ok())
            .or_else(|| level.and_then(|level| EnvFilter::try_new(level).ok()))
            .unwrap_or_else(|| EnvFilter::new(DEFAULT_LEVEL))
    };
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("pasta")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(dir);
    let (file_layer, guard) = match appender {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(writer).with_filter(filter());
            (Some(layer), Some(guard))
        }
        Err(err) => {
            eprintln!("pasta: can't write logs to {}: {err}", dir.display());
            (None, None)
        }
    };
    // Also log to the terminal in dev builds.
    let stderr_layer = cfg!(debug_assertions)
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_filter(filter()));
    let _ = tracing_subscriber::registry().with(file_layer).with(stderr_layer).try_init();
    guard
}
