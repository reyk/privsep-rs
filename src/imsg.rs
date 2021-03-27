//! Internal messages ("imsg") protocol between privilege-separated processes.

use crate::net::{AncillaryData, Fd, SocketAncillary, UnixStream, UnixStreamExt};
use derive_more::{From, Into};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    convert::TryFrom,
    io::{self, Result},
    mem,
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
};
use zerocopy::{AsBytes, FromBytes};

#[derive(Debug, From, Into)]
pub struct Handler {
    socket: UnixStream,
}

impl Handler {
    pub fn pair() -> Result<(Self, Self)> {
        UnixStream::pair().map(|(a, b)| (a.into(), b.into()))
    }

    pub fn from_raw_fd<T: IntoRawFd>(fd: T) -> Result<Handler> {
        unsafe { UnixStream::from_raw_fd(fd.into_raw_fd()).map(Into::into) }
    }

    pub async fn send_message<T: Serialize>(
        &self,
        mut message: Message,
        fd: Option<&Fd>,
        data: &T,
    ) -> Result<()> {
        let data = bincode::serialize(data)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
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

#[derive(Debug, AsBytes, FromBytes, Default)]
#[repr(C)]
pub struct Message {
    pub id: u32,
    pub length: u16,
    pub flags: u16,
    pub peer_id: u32,
    pub pid: libc::pid_t,
}

impl Message {
    pub fn new<T: Into<u32>>(id: T) -> Self {
        let length = mem::size_of::<Self>() as u16;
        Message {
            id: id.into(),
            pid: unsafe { libc::getpid() },
            length,
            ..Default::default()
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
