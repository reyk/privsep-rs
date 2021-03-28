use derive_more::{Display, From};
use std::{env, io, num};

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
    #[display(fmt = "{}", "_0")]
    VarError(env::VarError),
    #[display(fmt = "{}", "_0")]
    JoinError(tokio::task::JoinError),
}

impl std::error::Error for Error {}
