//! Internal message handling between privilege-separated processes.

use crate::net::{AncillaryData, Fd, SocketAncillary, UnixStream, UnixStreamExt};
use derive_more::{From, Into};
use nix::unistd::getpid;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    convert::TryFrom,
    io::{self, Result},
    mem,
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
};
use zerocopy::{AsBytes, FromBytes};

/// `imsg` handler.
#[derive(Debug, From, Into)]
pub struct Handler {
    /// Async half of a UNIX socketpair.
    socket: UnixStream,
}

impl Handler {
    /// Create new handler pair.
    pub fn pair() -> Result<(Self, Self)> {
        UnixStream::pair().map(|(a, b)| (a.into(), b.into()))
    }

    pub fn socketpair() -> Result<(Fd, Fd)> {
        let (a, b) = Self::pair()?;
        let fd_a = Fd::from(a.as_raw_fd());
        let fd_b = Fd::from(b.as_raw_fd());
        mem::forget(a);
        mem::forget(b);
        Ok((fd_a, fd_b))
    }

    /// Create half of a handler pair from a file descriptor.
    pub fn from_raw_fd<T: IntoRawFd>(fd: T) -> Result<Handler> {
        unsafe { UnixStream::from_raw_fd(fd.into_raw_fd()).map(Into::into) }
    }

    /// Send message to remote end.
    pub async fn send_message<T: Serialize>(
        &self,
        message: Message,
        fd: Option<&Fd>,
        data: &T,
    ) -> Result<()> {
        if message.id <= Message::RESERVED {
            return Err(io::Error::new(io::ErrorKind::Other, "Reserved message ID"));
        }
        self.send_message_internal(message, fd, data).await
    }

    /// Send message to the remote end.
    pub(crate) async fn send_message_internal<T: Serialize>(
        &self,
        mut message: Message,
        fd: Option<&Fd>,
        data: &T,
    ) -> Result<()> {
        let data = bincode::serialize(data)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        message.pid = getpid().as_raw();
        message.length = u16::try_from(data.len() + message.length as usize)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let message_length = message.length as usize;
        let iovs = [
            io::IoSlice::new(&message.as_bytes()),
            io::IoSlice::new(&data),
        ];
        let bufs = if data.is_empty() {
            &iovs[..1]
        } else {
            &iovs[..]
        };

        let mut ancillary_buffer = [0; 128];
        let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);
        if let Some(fd) = fd {
            if !ancillary.add_fds(&[fd.as_raw_fd()]) {
                return Err(io::Error::new(io::ErrorKind::Other, "failed to add fd"));
            }
        }

        let length = self
            .socket
            .send_vectored_with_ancillary(&bufs, &mut ancillary)
            .await?;

        if length != message_length {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short message"));
        }

        Ok(())
    }

    /// Receive message from the remote end.
    pub async fn recv_message<T: DeserializeOwned>(
        &self,
    ) -> Result<Option<(Message, Option<Fd>, T)>> {
        let mut ancillary_buffer = [0u8; 128];
        let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);

        let mut message = Message::default();
        let mut message_buf = message.as_bytes_mut();

        let mut buf = [0u8; 0xffff];
        let bufs = &mut [
            io::IoSliceMut::new(&mut message_buf),
            io::IoSliceMut::new(&mut buf[..]),
        ][..];

        let length = self
            .socket
            .recv_vectored_with_ancillary(bufs, &mut ancillary)
            .await?;
        if length == 0 {
            return Ok(None);
        }
        let message_length = message.length as usize;

        if length < mem::size_of::<Message>() || length < message_length {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short message"));
        }

        let result = bincode::deserialize(&buf)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        let mut fd_result = None;
        for ancillary_result in ancillary.messages().flatten() {
            #[allow(irrefutable_let_patterns)]
            if let AncillaryData::ScmRights(scm_rights) = ancillary_result {
                for fd in scm_rights {
                    let fd = Fd::from(fd);

                    // We only return one fd per message and
                    // auto-close all the remaining ones once the `Fd`
                    // is dropped.
                    if fd_result.is_none() {
                        fd_result = Some(fd);
                    }
                }
            }
        }

        Ok(Some((message, fd_result, result)))
    }
}

impl AsRawFd for Handler {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

/// Internal message header.
#[derive(Debug, AsBytes, FromBytes, Default)]
#[repr(C)]
pub struct Message {
    /// Request type.
    pub id: u32,
    /// Total message length (header + payload).
    pub length: u16,
    /// Optional flags.
    pub flags: u16,
    /// Optional peer ID.
    pub peer_id: u32,
    /// Local PID.
    pub pid: libc::pid_t,
}

impl Message {
    // Reserved IDs
    const RESERVED: u32 = 10;

    /// Create new message header.
    pub fn new<T: Into<u32>>(id: T) -> Self {
        let length = mem::size_of::<Self>() as u16;
        Message {
            id: id.into(),
            pid: getpid().as_raw(),
            length,
            ..Default::default()
        }
    }

    pub fn connect(peer_id: usize) -> Self {
        Self {
            peer_id: peer_id as u32,
            ..Self::new(1u32)
        }
    }
}

impl<T: Into<u32>> From<T> for Message {
    fn from(id: T) -> Self {
        Message::new(id)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_empty_data() {
        let data = bincode::serialize(&()).unwrap();
        assert!(data.is_empty());
    }
}
