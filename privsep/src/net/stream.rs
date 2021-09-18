//! `UnixStream` extensions to support file descriptor passing.

use crate::net::ancillary::{
    recv_vectored_with_ancillary_from, send_vectored_with_ancillary_to, SocketAncillary,
};
use async_trait::async_trait;
use std::{
    io::{self, IoSlice, IoSliceMut, Result},
    os::unix::{
        io::{FromRawFd, RawFd},
        net as std_net,
    },
};
use tokio::{net as tokio_net, task::yield_now};

pub use tokio_net::UnixStream;

#[async_trait]
pub trait UnixStreamExt {
    async fn recv_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSliceMut<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize>;

    async fn send_vectored_with_ancillary(
        &self,
        bufs: &[IoSlice<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize>;

    #[allow(clippy::missing_safety_doc)]
    unsafe fn from_raw_fd(fd: RawFd) -> Result<UnixStream>;
}

#[async_trait]
impl UnixStreamExt for UnixStream {
    async fn recv_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSliceMut<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize> {
        loop {
            self.readable().await?;

            match recv_vectored_with_ancillary_from(self, bufs, ancillary) {
                Ok((count, _)) => break Ok(count),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(err) => break Err(err),
            }
        }
    }

    async fn send_vectored_with_ancillary(
        &self,
        bufs: &[IoSlice<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize> {
        loop {
            self.writable().await?;

            match send_vectored_with_ancillary_to(self, bufs, ancillary) {
                Ok(count) => break Ok(count),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(err) => break Err(err),
            }
        }
    }

    unsafe fn from_raw_fd(fd: RawFd) -> Result<Self> {
        Self::from_std(std_net::UnixStream::from_raw_fd(fd))
    }
}

pub trait StdUnixStreamExt {
    fn recv_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSliceMut<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize>;

    fn send_vectored_with_ancillary(
        &self,
        bufs: &[IoSlice<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize>;
}

impl StdUnixStreamExt for std_net::UnixStream {
    fn recv_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSliceMut<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize> {
        match recv_vectored_with_ancillary_from(self, bufs, ancillary) {
            Ok((count, _)) => Ok(count),
            Err(err) => Err(err),
        }
    }

    fn send_vectored_with_ancillary(
        &self,
        bufs: &[IoSlice<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize> {
        send_vectored_with_ancillary_to(self, bufs, ancillary)
    }
}
