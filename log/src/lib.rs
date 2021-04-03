//! Simple async logging crate inspired by OpenBSD's `log.c`

use derive_more::{Display, From, Into};
use libc::openlog;
use serde_derive::{Deserialize, Serialize};
use slog::{Drain, Level, OwnedKVList, Record, KV};
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

/// Re-export the scoped logging macros.
pub use slog_scope::{debug, error, info, trace, warn};

static LOG_BRIDGE: Once = Once::new();

/// Configuration for the logging crate.
#[derive(Debug, Default, Deserialize, Serialize, From)]
pub struct Config {
    /// Log to the foreground or to syslog (default: syslog).
    #[from(forward)]
    foreground: bool,
}

/// Logging errors.
#[derive(Debug, Display, From)]
pub enum Error {
    #[display(fmt = "{}", "_0")]
    NulError(std::ffi::NulError),
    #[display(fmt = "{}", "_0")]
    IoError(io::Error),
    #[display(fmt = "{}", "_0")]
    SendError(mpsc::error::SendError<Message>),
}

impl std::error::Error for Error {}

fn init(
    drain: Box<dyn Drain<Err = slog::Never, Ok = ()> + Send>,
    _config: Config,
) -> GlobalLoggerGuard {
    let kv = slog::o!();

    // TODO: apply config overwrites
    let drain = slog_envlogger::new(drain);

    // This is required to make the drain `UnwindSafe`.
    let drain = Mutex::new(drain.fuse());

    let logger = slog::Logger::root(drain.fuse(), kv).into_erased();

    let guard = slog_scope::set_global_logger(logger);
    LOG_BRIDGE.call_once(|| {
        slog_stdlog::init().unwrap();
    });

    guard
}

/// Return a new global async logger.
pub async fn async_logger<C: Into<Config>>(
    name: &str,
    config: C,
) -> Result<GlobalLoggerGuard, Error> {
    let config = config.into();

    let drain = if config.foreground {
        Async::new(Box::new(Stderr::new(name)?)).await
    } else {
        Async::new(Box::new(Syslog::new(name)?)).await
    };

    Ok(init(Box::new(drain.fuse()), config))
}

/// Return a new global async logger.
pub fn sync_logger<C: Into<Config>>(name: &str, config: C) -> Result<GlobalLoggerGuard, Error> {
    let config = config.into();

    let guard = if config.foreground {
        init(Box::new(Stderr::new(name)?.fuse()), config)
    } else {
        init(Box::new(Syslog::new(name)?.fuse()), config)
    };

    Ok(guard)
}

/// Local trait that can be used by the async logger.
pub trait Target: Send + Sync {
    fn new(name: &str) -> Result<Self, Error>
    where
        Self: Sized;
    fn log_str(&self, name: &str) -> Result<(), Error>;
}

/// Forground logger that logs to stderr.
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
        let message = format!("{}: {}\n", self.name, message);
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

/// Background logger to log to syslog.
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

/// Async channel that sends log messages to a background task.
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
            .map_err(Into::into)
    }
}

impl Drop for Async {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.sender.send(Message::Close).unwrap();

            let waiter = thread::spawn(|| {
                if let Ok(runtime) = Runtime::new() {
                    runtime.block_on(async move {
                        let _ = time::timeout(Duration::from_secs(1), handle).await;
                    });
                }
            });
            waiter.join().expect("async logger");
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Entry(Level, String),
    Close,
}

pub struct AsyncLogger {
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
    use crate::{async_logger, debug, info};

    #[tokio::test(flavor = "multi_thread")]
    async fn test_log_stderr() {
        let _guard = async_logger("test", true).await.unwrap();

        for i in 1..=1000 {
            info!("Hello, World! {}", i);
            debug!("Hello, World! {}", i);
        }
    }
}
