//! File logging for the windowless tray app (spec 01 section 8).
//!
//! `TokenMon.exe` has no console, so stdout goes nowhere. This module writes
//! `tracing` events to a size-bounded rolling file under the data folder
//! (`%LOCALAPPDATA%\TokenMon\logs`) and installs a panic hook that records the
//! panic before the release profile's `panic = "abort"` ends the process.
//!
//! The log is for diagnosis, not telemetry: call sites must never pass
//! credentials, cookies, API keys or tokens to `tracing` macros, and the
//! files live in the per-user data folder.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tracing_subscriber::EnvFilter;

/// Name of the active log file; rolled files append `.1`, `.2`, ...
pub const LOG_FILE_NAME: &str = "tokenmon.log";

/// Sub-folder of the data directory that holds the logs.
pub const LOG_DIR_NAME: &str = "logs";

/// Largest size the active file reaches before it rolls.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Most files kept, the active one included.
pub const DEFAULT_MAX_FILES: usize = 5;

/// The log folder inside the app data directory `data_dir`.
///
/// ```
/// use std::path::Path;
/// use tokenmon_app::logging::log_dir;
/// assert!(log_dir(Path::new("data")).ends_with("logs"));
/// ```
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR_NAME)
}

struct State {
    file: File,
    size: u64,
}

/// A rolling log file: when the active file would pass `max_file_bytes` it is
/// renamed to `.1` (older ones shift up, the oldest beyond `max_files` is
/// deleted) and a fresh file starts. Every write goes straight to the file, so
/// nothing is buffered when the process aborts.
pub struct RollingFile {
    dir: PathBuf,
    max_file_bytes: u64,
    max_files: usize,
    state: Mutex<State>,
}

impl RollingFile {
    /// Opens (creating the folder and file) the log in `dir`.
    ///
    /// # Errors
    /// The folder or file could not be created.
    pub fn open(dir: &Path, max_file_bytes: u64, max_files: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE_NAME))?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            max_file_bytes: max_file_bytes.max(1),
            max_files: max_files.max(1),
            state: Mutex::new(State { file, size }),
        })
    }

    fn rolled(&self, index: usize) -> PathBuf {
        self.dir.join(format!("{LOG_FILE_NAME}.{index}"))
    }

    /// Shifts `.n` to `.n+1`, drops the oldest and reopens an empty active file.
    fn roll(&self, state: &mut State) -> io::Result<()> {
        let keep_rolled = self.max_files - 1;
        let _ = fs::remove_file(self.rolled(keep_rolled.max(1)));
        for i in (1..keep_rolled).rev() {
            let _ = fs::rename(self.rolled(i), self.rolled(i + 1));
        }
        let active = self.dir.join(LOG_FILE_NAME);
        if keep_rolled == 0 {
            let _ = fs::remove_file(&active);
        } else {
            let _ = fs::rename(&active, self.rolled(1));
        }
        state.file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&active)?;
        state.size = 0;
        Ok(())
    }

    fn write_bytes(&self, buf: &[u8]) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let len = buf.len() as u64;
        if state.size > 0 && state.size + len > self.max_file_bytes {
            // A failed roll keeps appending to the old file rather than lose lines.
            let _ = self.roll(&mut state);
        }
        state.file.write_all(buf)?;
        state.size += len;
        Ok(())
    }
}

/// The `io::Write` handle `tracing` gets for each event.
#[derive(Clone)]
pub struct LogWriter(Arc<RollingFile>);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_bytes(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A subscriber that writes to `file`. `RUST_LOG` overrides the default
/// `info` level.
pub fn file_subscriber(file: &Arc<RollingFile>) -> impl tracing::Subscriber + Send + Sync {
    let writer = LogWriter(Arc::clone(file));
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish()
}

/// Starts logging to `<data_dir>/logs/tokenmon.log` and installs the panic
/// hook. A second call in one process is ignored.
///
/// # Errors
/// The log folder or file could not be created; the caller keeps running
/// without a log.
pub fn init_file_logging(data_dir: &Path) -> io::Result<PathBuf> {
    let dir = log_dir(data_dir);
    let file = Arc::new(RollingFile::open(
        &dir,
        DEFAULT_MAX_FILE_BYTES,
        DEFAULT_MAX_FILES,
    )?);
    let _ = tracing::subscriber::set_global_default(file_subscriber(&file));
    install_panic_hook();
    Ok(dir)
}

/// The text logged for a panic: its message and where it happened.
///
/// ```
/// use tokenmon_app::logging::describe_panic;
/// let payload = std::panic::catch_unwind(|| panic!("boom")).unwrap_err();
/// let text = describe_panic(payload.as_ref(), None);
/// assert_eq!(text, "panic: boom");
/// ```
pub fn describe_panic(
    payload: &(dyn std::any::Any + Send),
    location: Option<&Location<'_>>,
) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned());
    match location {
        Some(l) => format!(
            "panic: {message} at {}:{}:{}",
            l.file(),
            l.line(),
            l.column()
        ),
        None => format!("panic: {message}"),
    }
}

/// Logs every panic (with its location) through `tracing`, then runs the
/// previous hook. Release builds abort on panic, so this line is the only
/// evidence of what killed the app.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            "{}",
            describe_panic(info.payload(), info.location())
        );
        previous(info);
    }));
}
