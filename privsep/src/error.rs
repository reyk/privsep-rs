use derive_more::{Display, From};
use std::{borrow::Cow, env, io, num};

/// Common errors.
#[derive(Debug, Display, From)]
pub enum Error {
    #[display(fmt = "I/O error: {}", "_0")]
    IoError(io::Error),
    #[display(fmt = "Permission denied, must run as root")]
    PermissionDenied,
    #[display(fmt = "{}", "_0")]
    UnixError(nix::Error),
    #[display(fmt = "{:?}", "_0")]
    Error(&'static str),
    #[display(fmt = "{}", "_0")]
    InvalidArgument(num::ParseIntError),
    #[display(fmt = "Invalid process")]
    InvalidProcess,
    #[display(fmt = "{}", "_0")]
    VarError(env::VarError),
    #[display(fmt = "{}", "_0")]
    JoinError(tokio::task::JoinError),
    #[display(fmt = "Username '{}' for dropping privileges not found", "_0")]
    UserNotFound(Cow<'static, str>),
    #[display(fmt = "Failed to drop privileges ({}) - {}", "_0", "_1")]
    Privdrop(&'static str, Box<dyn std::error::Error>),
}

impl std::error::Error for Error {}
