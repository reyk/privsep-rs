//! Simple async logging crate inspired by OpenBSD's `log.c`

use derive_more::{Display, From};
use libc::openlog;
use slog::{Drain, Level, OwnedKVList, Record, KV};
use slog_async::Async;
use slog_scope::GlobalLoggerGuard;
use std::{
    ffi::{CStr, CString},
    fmt,
    io::{self, Write},
    sync::{Mutex, Once},
};

pub use slog_scope::{debug, error, info, trace, warn};

static LOG_BRIDGE: Once = Once::new();

#[derive(Debug, Display, From)]
pub enum Error {
    #[display(fmt = "{}", "_0")]
    NulError(std::ffi::NulError),
    #[display(fmt = "{}", "_0")]
    IoError(io::Error),
}

impl std::error::Error for Error {}

pub fn init(name: &str, foreground: bool) -> Result<GlobalLoggerGuard, Error> {
    let kv = slog::o!();

    let drain = if foreground {
        Async::new(Stderr::new(name)?.fuse()).build()
    } else {
        Async::new(Syslog::new(name)?.fuse()).build()
    };
    let drain = slog_envlogger::new(drain);
    let drain = Mutex::new(drain.fuse());

    let logger = slog::Logger::root(drain.fuse(), kv).into_erased();

    let guard = slog_scope::set_global_logger(logger);
    LOG_BRIDGE.call_once(|| {
        slog_stdlog::init().unwrap();
    });

    Ok(guard)
}

pub trait Target: Send + Sync {
    fn new(name: &str) -> Result<Self, Error>
    where
        Self: Sized;
    fn log_str(&self, name: &str) -> Result<(), Error>;
}

pub struct Stderr {
    name: String,
}

impl Target for Stderr {
    fn new(name: &str) -> Result<Self, Error> {
        Ok(Self {
            name: name.to_string(),
        })
    }

    fn log_str(&self, message: &str) -> Result<(), Error> {
        io::stderr()
            .write_all(message.as_bytes())
            .map_err(Into::into)
    }
}

impl Drain for Stderr {
    type Ok = ();
    type Err = Error;

    fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        let message =
            format!("{} ", record.level()) + &format_log(Some(&self.name), record, values) + "\n";
        self.log_str(&message)
    }
}

// TODO: use the reentrant version
pub struct Syslog {
    // we need to keep a reference to the const char * around.
    _name: CString,
}

impl Target for Syslog {
    fn new(name: &str) -> Result<Self, Error> {
        let name = name.to_string();
        let _name: CString = CString::new(&name[..name.find('(').unwrap_or_else(|| name.len())])?;
        let c_str: &CStr = _name.as_c_str();

        unsafe {
            openlog(
                c_str.as_ptr(),
                libc::LOG_PID | libc::LOG_NDELAY,
                libc::LOG_DAEMON,
            )
        };

        Ok(Self { _name })
    }

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
        let message = format_log(None, record, values);
        self.log_str(&message)
    }
}

fn format_log(name: Option<&str>, record: &Record<'_>, values: &OwnedKVList) -> String {
    let mut formatter = Formatter::new(name, record);
    let _ = record.kv().serialize(record, &mut formatter);
    let _ = values.serialize(record, &mut formatter);
    formatter.buf
}

struct Formatter {
    pub buf: String,
}

impl Formatter {
    fn new(name: Option<&str>, record: &Record<'_>) -> Self {
        let mut buf = if let Some(name) = name {
            format!("{}: {}", name, record.msg())
        } else {
            format!("{}", record.msg())
        };

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

impl slog::Serializer for Formatter {
    fn emit_arguments(&mut self, key: &str, val: &fmt::Arguments<'_>) -> slog::Result {
        self.buf.push_str(&format!(", {}: {}", key, val));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{debug, info, init};

    #[test]
    fn test_log_stderr() {
        let _guard = init("test", true).unwrap();

        info!("Hello, World!");
        debug!("Hello, World!");
    }
}
