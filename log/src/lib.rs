//! Simple async logging crate inspired by OpenBSD's `log.c`.
//!
//! # Examples
//!
//! The logger is typically started from the programs's main function:
//!
//! ```
//! # use privsep_log::info;
//! let _guard = privsep_log::sync_logger("test", true).unwrap();
//!
//! info!("Started");
//! ```
//!
//! The async logger is provided to run inside of a [`tokio`] runtime.
//!
//! ```
//! use privsep_log::info;
//!
//! #[tokio::main]
//! async fn main() {
//!     let _guard = privsep_log::async_logger("test", true).await.unwrap();
//!
//!     info!("Started");
//! }
//! ```
//!
//! The `sync_logger` and `async_logger` functions take two arguments:
//! the name of the program and the options, an arbitrary variable
//! that implements `Into<Config>`. A simple boolean can be passed to
//! indicate if logging should happen in foreground via standard error
//! output or in the background via syslog; this is identical to
//! passing the `Config` struct with the `foreground` option:
//!
//! ```
//! # use privsep_log::{Config, debug, info};
//! // Lazily initialize the default logger.
//! privsep_log::init();
//!
//! // Parse configuration or command-line options...
//! let args = std::env::args().collect::<Vec<_>>();
//!
//! debug!("Reading configuration...");
//! let config = Config {
//!     foreground: false,
//!     ..Default::default()
//! };
//! let name = &args[0];
//!
//! // Now start the background logger.
//! let _guard = privsep_log::sync_logger(name, config).unwrap();
//!
//! info!("Started");
//! ```
//!
//! [`tokio`]: https://tokio.rs

use derive_more::{Display, From, Into};
use libc::openlog;
use serde_derive::{Deserialize, Serialize};
use slog::{Drain, Level, Logger, OwnedKVList, Record, KV};
use slog_scope::GlobalLoggerGuard;
use std::{
    ffi::{CStr, CString},
    fmt,
    io::{self, Write},
    pin::Pin,
    sync::{Mutex, Once},
    thread,
    time::Duration,
};
use tokio::{runtime::Runtime, sync::mpsc, time};

mod envlogger;

pub use slog_scope::{debug, error, info, trace, warn};

static LOG_BRIDGE: Once = Once::new();

lazy_static::lazy_static! {
    /// Default logger global guard.
    ///
    /// This is used before a logger context is initialized.
    static ref GLOBAL_LOGGER_GUARD: (Logger, GlobalLoggerGuard) = {
        new(
            Box::new(Stderr::new("").unwrap().fuse()),
            Config {
                foreground: true,
                filter: Some("debug".to_string()),
            }
        )
    };

    /// Default global logger scope.
    static ref GLOBAL_LOGGER: Logger = GLOBAL_LOGGER_GUARD.0.clone();
}

/// Configuration for the logging crate.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    /// Log to the foreground or to syslog (default: syslog).
    pub foreground: bool,
    /// Log filter (can be overridden by the `RUST_LOG` environment variable).
    pub filter: Option<String>,
}

impl From<bool> for Config {
    #[inline]
    fn from(foreground: bool) -> Self {
        Self {
            foreground,
            ..Default::default()
        }
    }
}

/// Logging errors.
#[derive(Debug, Display, From)]
pub enum Error {
    #[display(fmt = "{}", "_0")]
    NulError(std::ffi::NulError),
    #[display(fmt = "{}", "_0")]
    IoError(io::Error),
    #[display(fmt = "{}", "_0")]
    SendError(String),
}

impl std::error::Error for Error {}

/// Initialize the global logger context.
///
/// This also called by `sync_logger` and `async_logger`.
pub fn init() {
    lazy_static::initialize(&GLOBAL_LOGGER);
    LOG_BRIDGE.call_once(|| {
        if let Err(err) = slog_stdlog::init() {
            error!("Failed to initialize log bridge: {}", err);
        }
    });
}

fn new(
    drain: Box<dyn Drain<Err = slog::Never, Ok = ()> + Send>,
    config: Config,
) -> (Logger, GlobalLoggerGuard) {
    let kv = slog::o!();

    // Build log filter
    let drain =
        envlogger::Logger::with_default_filter(drain, config.filter.as_deref().unwrap_or("info"));

    // This is required to make the drain `UnwindSafe`.
    let drain = Mutex::new(drain.fuse());

    let logger = slog::Logger::root(drain.fuse(), kv).into_erased();
    let guard = slog_scope::set_global_logger(logger.clone());

    (logger, guard)
}

/// Return a new global async logger.
pub async fn async_logger<N: AsRef<str>, C: Into<Config>>(
    name: N,
    config: C,
) -> Result<LoggerGuard, Error> {
    let config = config.into();

    init();

    let name = name.as_ref();
    let drain = if config.foreground {
        Async::new(Box::new(Stderr::new(name)?)).await
    } else {
        Async::new(Box::new(Syslog::new(name)?)).await
    };

    Ok(new(Box::new(drain.fuse()), config).into())
}

/// Return a new global sync logger.
pub fn sync_logger<N: AsRef<str>, C: Into<Config>>(
    name: N,
    config: C,
) -> Result<LoggerGuard, Error> {
    let config = config.into();

    init();

    let name = name.as_ref();
    let guard = if config.foreground {
        new(Box::new(Stderr::new(name)?.fuse()), config)
    } else {
        new(Box::new(Syslog::new(name)?.fuse()), config)
    };

    Ok(guard.into())
}

/// Wrapper for the global logger guard.
#[derive(From)]
pub struct LoggerGuard {
    _logger: Logger,
    _guard: GlobalLoggerGuard,
}

impl Drop for LoggerGuard {
    fn drop(&mut self) {
        let guard = slog_scope::set_global_logger(GLOBAL_LOGGER.clone());
        guard.cancel_reset();
    }
}

/// Local trait that can be used by the async logger.
pub trait Target: Send + Sync {
    fn new(name: &str) -> Result<Self, Error>
    where
        Self: Sized;
    fn log_str(&self, name: &str) -> Result<(), Error>;
}

/// Forground logger drain that logs to stderr.
pub struct Stderr {
    name: String,
}

impl Target for Stderr {
    /// Create a new foreground logger.
    fn new(name: &str) -> Result<Self, Error> {
        Ok(Self {
            name: name.to_string(),
        })
    }

    /// Log the pre-formatted string.
    fn log_str(&self, message: &str) -> Result<(), Error> {
        let message = if !self.name.is_empty() {
            format!("{}: {}\n", self.name, message)
        } else {
            format!("{}\n", message)
        };
        io::stderr()
            .write_all(message.as_bytes())
            .map_err(Into::into)
    }
}

impl Drain for Stderr {
    type Ok = ();
    type Err = Error;

    fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        let message = format_log(record, values);
        self.log_str(&message)
    }
}

/// Background logger drain to log to syslog.
// TODO: use the reentrant version
pub struct Syslog {
    /// We need to keep a reference to the const char * around.
    _name: Pin<CString>,
}

impl Target for Syslog {
    /// Create a new background logger.
    fn new(name: &str) -> Result<Self, Error> {
        let name = name.to_string();
        let _name = CString::new(&name[..name.find('(').unwrap_or_else(|| name.len())])?;
        let c_str: &CStr = _name.as_c_str();

        unsafe {
            openlog(
                c_str.as_ptr(),
                libc::LOG_PID | libc::LOG_NDELAY,
                libc::LOG_DAEMON,
            )
        };

        Ok(Self {
            _name: Pin::new(_name),
        })
    }

    /// Convert the log string into a syslog message.
    fn log_str(&self, message: &str) -> Result<(), Error> {
        let c_string: CString = CString::new(message.as_bytes())?;
        let c_message: &CStr = c_string.as_c_str();

        let level = match Level::Info {
            Level::Critical => libc::LOG_CRIT,
            Level::Error => libc::LOG_ERR,
            Level::Warning => libc::LOG_WARNING,
            Level::Info => libc::LOG_INFO,
            Level::Debug | Level::Trace => libc::LOG_DEBUG,
        };

        unsafe {
            libc::syslog(level, c_message.as_ptr());
        }

        Ok(())
    }
}

impl Drop for Syslog {
    /// Close syslog on shutdown.
    fn drop(&mut self) {
        unsafe {
            libc::closelog();
        }
    }
}

impl Drain for Syslog {
    type Ok = ();
    type Err = Error;

    fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        let message = format_log(record, values);
        self.log_str(&message)
    }
}

/// Async logger drain that sends log messages to a background task.
pub struct Async {
    sender: mpsc::UnboundedSender<Message>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Async {
    /// Create new async logger that holds one of the supported target loggers.
    pub async fn new(target: Box<dyn Target>) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel::<Message>();

        let handle = tokio::spawn(async move {
            let mut logger = AsyncLogger::new(receiver, target);
            logger.listen().await;
        });

        Self {
            sender,
            handle: Some(handle),
        }
    }
}

impl Drain for Async {
    type Ok = ();
    type Err = Error;

    fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        let message = format_log(record, values);
        self.sender
            .send(Message::Entry(record.level(), message))
            .map_err(|err| Error::SendError(err.to_string()))
    }
}

impl Drop for Async {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let waiter = thread::spawn(|| {
                if let Ok(runtime) = Runtime::new() {
                    runtime.block_on(async move {
                        let _ = time::timeout(Duration::from_secs(1), handle).await;
                    });
                }
            });

            self.sender.send(Message::Close).unwrap();

            waiter.join().expect("async logger");
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    Entry(Level, String),
    Close,
}

struct AsyncLogger {
    receiver: mpsc::UnboundedReceiver<Message>,
    target: Box<dyn Target>,
}

impl AsyncLogger {
    pub fn new(receiver: mpsc::UnboundedReceiver<Message>, target: Box<dyn Target>) -> Self {
        Self { receiver, target }
    }

    pub async fn listen(&mut self) {
        while let Some(Message::Entry(_level, message)) = self.receiver.recv().await {
            // TODO: count errors or abort.
            let _ = self.target.log_str(&message);
        }
    }
}

/// Format the log message to a string.
#[inline]
fn format_log(record: &Record<'_>, values: &OwnedKVList) -> String {
    let mut formatter = Formatter::new(record);
    let _ = record.kv().serialize(record, &mut formatter);
    let _ = values.serialize(record, &mut formatter);
    formatter.into()
}

/// Formatter to create a log message from a record.
#[derive(Into)]
struct Formatter {
    #[into]
    buf: String,
}

impl Formatter {
    /// Return a new formatter.
    fn new(record: &Record<'_>) -> Self {
        let mut buf = format!("{}", record.msg());

        if record.level() >= Level::Debug {
            // Rust does not support function!()
            buf.push_str(&format!(
                ", source: {}:{}, module: {}",
                record.file(),
                record.line(),
                record.module()
            ));
        };

        Self { buf }
    }
}

/// Serializer for key-value fields.
impl slog::Serializer for Formatter {
    fn emit_arguments(&mut self, key: &str, val: &fmt::Arguments<'_>) -> slog::Result {
        self.buf.push_str(&format!(", {}: {}", key, val));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{async_logger, debug, info, init, Config};

    #[test]
    fn test_default_log() {
        init();
        info!("default log");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_log_stderr() {
        let _guard = async_logger(
            "test",
            Config {
                foreground: true,
                filter: Some("debug".to_string()),
            },
        )
        .await
        .unwrap();

        for i in 1..=100 {
            info!("Hello, World! {}", i);
            debug!("Hello, World! {}", i);
        }
    }
}
