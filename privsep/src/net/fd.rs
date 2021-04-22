//! Owned, droppable file descriptors.

use derive_more::{From, Into};
use nix::fcntl::{fcntl, FcntlArg};
use std::{
    io::{self, Result},
    mem,
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
};

/// Wrapper for `RawFd` that closes the file descriptor when dropped.
#[derive(Debug, From, Into)]
pub struct Fd(RawFd);

impl Fd {
    /// Duplicate the file descriptor into an independent `Fd`.
    pub fn duplicate(&self) -> Result<Self> {
        match unsafe { libc::dup(self.0) } {
            -1 => Err(io::Error::last_os_error()),
            fd => Ok(fd.into()),
        }
    }

    /// Check if the file descriptor is valid,
    pub fn is_open(&self) -> Result<()> {
        fcntl(self.0, FcntlArg::F_GETFD)
            .map(|_| ())
            .map_err(|err| io::Error::new(io::ErrorKind::NotConnected, err))
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
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
