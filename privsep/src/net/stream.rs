use crate::net::ancillary::{
    recv_vectored_with_ancillary_from, send_vectored_with_ancillary_to, SocketAncillary,
};
use derive_more::{Deref, DerefMut, From};
use std::{
    io::{self, IoSlice, IoSliceMut, Result},
    os::unix::{
        io::{FromRawFd, RawFd},
        net as std_net,
    },
    path::Path,
};
use tokio::net as tokio_net;

#[derive(From, Deref, DerefMut)]
pub struct UnixStream(tokio_net::UnixStream);

impl UnixStream {
    pub async fn connect<P>(path: P) -> Result<UnixStream>
    where
        P: AsRef<Path>,
    {
        tokio_net::UnixStream::connect(path).await.map(Into::into)
    }

    pub fn pair() -> Result<(UnixStream, UnixStream)> {
        tokio_net::UnixStream::pair().map(|(a, b)| (a.into(), b.into()))
    }

    pub fn from_std(stream: std_net::UnixStream) -> Result<UnixStream> {
        tokio_net::UnixStream::from_std(stream).map(Into::into)
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_raw_fd(fd: RawFd) -> Result<UnixStream> {
        Self::from_std(std_net::UnixStream::from_raw_fd(fd))
    }
}

pub trait UnixStreamExt {
    fn recv_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSliceMut<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize>;

    fn send_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSlice<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize>;
}

impl UnixStreamExt for std_net::UnixStream {
    fn recv_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSliceMut<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize> {
        match recv_vectored_with_ancillary_from(self, bufs, ancillary) {
            Ok((_, true)) => {
                // TODO: handle truncation, try again.
                Err(io::Error::new(io::ErrorKind::Other, "truncated"))
            }
            Ok((count, false)) => Ok(count),
            Err(err) => Err(err),
        }
    }

    fn send_vectored_with_ancillary(
        &self,
        bufs: &mut [IoSlice<'_>],
        ancillary: &mut SocketAncillary<'_>,
    ) -> Result<usize> {
        send_vectored_with_ancillary_to(self, bufs, ancillary)
    }
}
