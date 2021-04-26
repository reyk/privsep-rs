//! Owned, droppable file descriptors.

use crate::error::Error;
use derive_more::{From, Into};
use nix::{
    fcntl::{fcntl, FcntlArg},
    unistd::{close, dup},
};
use std::{
    io::{self},
    mem,
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
};

/// Wrapper for `RawFd` that closes the file descriptor when dropped.
#[derive(Debug, From, Into)]
pub struct Fd(RawFd);

impl Fd {
    /// Duplicate the file descriptor into an independent `Fd`.
    pub fn duplicate(&self) -> Result<Self, Error> {
        dup(self.0).map(Self::from).map_err(Error::from)
    }

    /// Check if the file descriptor is valid,
    pub fn is_open(&self) -> Result<(), Error> {
        fcntl(self.0, FcntlArg::F_GETFD)
            .map(|_| ())
            .map_err(|err| io::Error::new(io::ErrorKind::NotConnected, err).into())
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        let _ = close(self.0);
    }
}

impl IntoRawFd for Fd {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.0;
        mem::forget(self);
        fd
    }
}

impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}
