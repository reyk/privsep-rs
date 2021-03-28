//! Unix socket Ancillary data handling.
//!
//! The code is based on "unstable" nightly-only code from the Rust
//! std library.  It is modified to work outside the std library and
//! to work include support for macOS (not present in the current
//! version as of this writing).
//!
//! Original source:
//! https://raw.githubusercontent.com/rust-lang/rust/master/library/std/src/sys/unix/ext/net/ancillary.rs
//!
//! Licensed under the MIT license:
//!
//! Permission is hereby granted, free of charge, to any
//! person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the
//! Software without restriction, including without
//! limitation the rights to use, copy, modify, merge,
//! publish, distribute, sublicense, and/or sell copies of
//! the Software, and to permit persons to whom the Software
//! is furnished to do so, subject to the following
//! conditions:
//!
//! The above copyright notice and this permission notice
//! shall be included in all copies or substantial portions
//! of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
//! ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
//! TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//! PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
//! SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
//! IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! DEALINGS IN THE SOFTWARE.

use std::{
    convert::TryFrom,
    io::{self, IoSlice, IoSliceMut},
    marker::PhantomData,
    mem::{size_of, zeroed},
    os::unix::io::{AsRawFd, RawFd},
    ptr::{eq, read_unaligned},
    slice::from_raw_parts,
};

// FIXME(#43348): Make libc adapt #[doc(cfg(...))] so we don't need these fake definitions here?
#[cfg(all(doc, not(target_os = "linux"), not(target_os = "android")))]
#[allow(non_camel_case_types)]
mod libc {
    pub use libc::c_int;
    pub struct ucred;
    pub struct cmsghdr;
    pub type pid_t = i32;
    pub type gid_t = u32;
    pub type uid_t = u32;
}

pub(super) fn recv_vectored_with_ancillary_from<S: AsRawFd>(
    socket: &S,
    bufs: &mut [IoSliceMut<'_>],
    ancillary: &mut SocketAncillary<'_>,
) -> io::Result<(usize, bool)> {
    unsafe {
        let mut msg: libc::msghdr = zeroed();

        cfg_if::cfg_if! {
            if #[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))] {
                msg.msg_iovlen = bufs.len() as libc::size_t;
                msg.msg_controllen = ancillary.buffer.len() as libc::size_t;
            } else if #[cfg(any(
                          target_os = "dragonfly",
                          target_os = "emscripten",
                          target_os = "freebsd",
                          target_os = "macos",
                          all(target_os = "linux", target_env = "musl",),
                          target_os = "netbsd",
                          target_os = "openbsd",
                      ))] {
                msg.msg_iovlen = bufs.len() as libc::c_int;
                msg.msg_controllen = ancillary.buffer.len() as libc::socklen_t;
            }
        }

        msg.msg_iov = bufs.as_mut_ptr().cast();
        if msg.msg_controllen > 0 {
            msg.msg_control = ancillary.buffer.as_mut_ptr().cast();
        }

        let count = match libc::recvmsg(socket.as_raw_fd(), &mut msg, 0) {
            -1 => Err(io::Error::last_os_error()),
            count => Ok(count as usize),
        }?;

        ancillary.length = msg.msg_controllen as usize;
        ancillary.truncated = msg.msg_flags & libc::MSG_CTRUNC == libc::MSG_CTRUNC;

        let truncated = msg.msg_flags & libc::MSG_TRUNC == libc::MSG_TRUNC;

        Ok((count, truncated))
    }
}

pub(super) fn send_vectored_with_ancillary_to<S: AsRawFd>(
    socket: &S,
    bufs: &[IoSlice<'_>],
    ancillary: &mut SocketAncillary<'_>,
) -> io::Result<usize> {
    unsafe {
        let mut msg: libc::msghdr = zeroed();

        cfg_if::cfg_if! {
            if #[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))] {
                msg.msg_iovlen = bufs.len() as libc::size_t;
                msg.msg_controllen = ancillary.length as libc::size_t;
            } else if #[cfg(any(
                          target_os = "dragonfly",
                          target_os = "emscripten",
                          target_os = "freebsd",
                          target_os = "macos",
                          all(target_os = "linux", target_env = "musl",),
                          target_os = "netbsd",
                          target_os = "openbsd",
                      ))] {
                msg.msg_iovlen = bufs.len() as libc::c_int;
                msg.msg_controllen = ancillary.length as libc::socklen_t;
            }
        }

        msg.msg_iov = bufs.as_ptr() as *mut _;
        if msg.msg_controllen > 0 {
            msg.msg_control = ancillary.buffer.as_mut_ptr().cast();
        }

        ancillary.truncated = false;

        match libc::sendmsg(socket.as_raw_fd(), &msg, 0) {
            -1 => Err(io::Error::last_os_error()),
            count => Ok(count as usize),
        }
    }
}

fn add_to_ancillary_data<T>(
    buffer: &mut [u8],
    length: &mut usize,
    source: &[T],
    cmsg_level: libc::c_int,
    cmsg_type: libc::c_int,
) -> bool {
    let source_len = if let Some(source_len) = source.len().checked_mul(size_of::<T>()) {
        if let Ok(source_len) = u32::try_from(source_len) {
            source_len
        } else {
            return false;
        }
    } else {
        return false;
    };

    unsafe {
        let additional_space = libc::CMSG_SPACE(source_len) as usize;

        let new_length = if let Some(new_length) = additional_space.checked_add(*length) {
            new_length
        } else {
            return false;
        };

        if new_length > buffer.len() {
            return false;
        }

        buffer[*length..new_length].fill(0);

        *length = new_length;

        let mut msg: libc::msghdr = zeroed();

        cfg_if::cfg_if! {
            if #[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))] {
                msg.msg_controllen = *length as libc::size_t;
            } else if #[cfg(any(
                          target_os = "dragonfly",
                          target_os = "emscripten",
                          target_os = "freebsd",
                          target_os = "macos",
                          all(target_os = "linux", target_env = "musl",),
                          target_os = "netbsd",
                          target_os = "openbsd",
                      ))] {
                msg.msg_controllen = *length as libc::socklen_t;
            }
        }
        if msg.msg_controllen > 0 {
            msg.msg_control = buffer.as_mut_ptr().cast();
        }

        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        let mut previous_cmsg = cmsg;
        while !cmsg.is_null() {
            previous_cmsg = cmsg;
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);

            // Most operating systems, but not Linux or emscripten, return the previous pointer
            // when its length is zero. Therefore, check if the previous pointer is the same as
            // the current one.
            if eq(cmsg, previous_cmsg) {
                break;
            }
        }

        if previous_cmsg.is_null() {
            return false;
        }

        (*previous_cmsg).cmsg_level = cmsg_level;
        (*previous_cmsg).cmsg_type = cmsg_type;
        cfg_if::cfg_if! {
            if #[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))] {
                (*previous_cmsg).cmsg_len = libc::CMSG_LEN(source_len) as libc::size_t;
            } else if #[cfg(any(
                          target_os = "dragonfly",
                          target_os = "emscripten",
                          target_os = "freebsd",
                          target_os = "macos",
                          all(target_os = "linux", target_env = "musl",),
                          target_os = "netbsd",
                          target_os = "openbsd",
                      ))] {
                (*previous_cmsg).cmsg_len = libc::CMSG_LEN(source_len) as libc::socklen_t;
            }
        }

        let data = libc::CMSG_DATA(previous_cmsg).cast();

        libc::memcpy(data, source.as_ptr().cast(), source_len as usize);
    }
    true
}

struct AncillaryDataIter<'a, T> {
    data: &'a [u8],
    phantom: PhantomData<T>,
}

impl<'a, T> AncillaryDataIter<'a, T> {
    /// Create `AncillaryDataIter` struct to iterate through the data unit in the control message.
    ///
    /// # Safety
    ///
    /// `data` must contain a valid control message.
    unsafe fn new(data: &'a [u8]) -> AncillaryDataIter<'a, T> {
        AncillaryDataIter {
            data,
            phantom: PhantomData,
        }
    }
}

impl<'a, T> Iterator for AncillaryDataIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if size_of::<T>() <= self.data.len() {
            unsafe {
                let unit = read_unaligned(self.data.as_ptr().cast());
                self.data = &self.data[size_of::<T>()..];
                Some(unit)
            }
        } else {
            None
        }
    }
}

/// Unix credential.
#[cfg(any(doc, target_os = "android", target_os = "linux",))]
#[derive(Clone)]
pub struct SocketCred(libc::ucred);

#[cfg(any(doc, target_os = "android", target_os = "linux",))]
impl SocketCred {
    /// Create a Unix credential struct.
    ///
    /// PID, UID and GID is set to 0.
    #[allow(clippy::new_without_default)]
    pub fn new() -> SocketCred {
        SocketCred(libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        })
    }

    /// Set the PID.
    pub fn set_pid(&mut self, pid: libc::pid_t) {
        self.0.pid = pid;
    }

    /// Get the current PID.
    pub fn get_pid(&self) -> libc::pid_t {
        self.0.pid
    }

    /// Set the UID.
    pub fn set_uid(&mut self, uid: libc::uid_t) {
        self.0.uid = uid;
    }

    /// Get the current UID.
    pub fn get_uid(&self) -> libc::uid_t {
        self.0.uid
    }

    /// Set the GID.
    pub fn set_gid(&mut self, gid: libc::gid_t) {
        self.0.gid = gid;
    }

    /// Get the current GID.
    pub fn get_gid(&self) -> libc::gid_t {
        self.0.gid
    }
}

/// This control message contains file descriptors.
///
/// The level is equal to `SOL_SOCKET` and the type is equal to `SCM_RIGHTS`.
pub struct ScmRights<'a>(AncillaryDataIter<'a, RawFd>);

impl<'a> Iterator for ScmRights<'a> {
    type Item = RawFd;

    fn next(&mut self) -> Option<RawFd> {
        self.0.next()
    }
}

/// This control message contains unix credentials.
///
/// The level is equal to `SOL_SOCKET` and the type is equal to `SCM_CREDENTIALS` or `SCM_CREDS`.
#[cfg(any(doc, target_os = "android", target_os = "linux",))]
pub struct ScmCredentials<'a>(AncillaryDataIter<'a, libc::ucred>);

#[cfg(any(doc, target_os = "android", target_os = "linux",))]
impl<'a> Iterator for ScmCredentials<'a> {
    type Item = SocketCred;

    fn next(&mut self) -> Option<SocketCred> {
        Some(SocketCred(self.0.next()?))
    }
}

/// The error type which is returned from parsing the type a control message.
#[non_exhaustive]
#[derive(Debug)]
pub enum AncillaryError {
    Unknown { cmsg_level: i32, cmsg_type: i32 },
}

/// This enum represent one control message of variable type.
pub enum AncillaryData<'a> {
    ScmRights(ScmRights<'a>),
    #[cfg(any(doc, target_os = "android", target_os = "linux",))]
    ScmCredentials(ScmCredentials<'a>),
}

impl<'a> AncillaryData<'a> {
    /// Create a `AncillaryData::ScmRights` variant.
    ///
    /// # Safety
    ///
    /// `data` must contain a valid control message and the control message must be type of
    /// `SOL_SOCKET` and level of `SCM_RIGHTS`.
    #[allow(clippy::wrong_self_convention)]
    unsafe fn as_rights(data: &'a [u8]) -> Self {
        let ancillary_data_iter = AncillaryDataIter::new(data);
        let scm_rights = ScmRights(ancillary_data_iter);
        AncillaryData::ScmRights(scm_rights)
    }

    /// Create a `AncillaryData::ScmCredentials` variant.
    ///
    /// # Safety
    ///
    /// `data` must contain a valid control message and the control message must be type of
    /// `SOL_SOCKET` and level of `SCM_CREDENTIALS` or `SCM_CREDENTIALS`.
    #[cfg(any(doc, target_os = "android", target_os = "linux",))]
    #[allow(clippy::wrong_self_convention)]
    unsafe fn as_credentials(data: &'a [u8]) -> Self {
        let ancillary_data_iter = AncillaryDataIter::new(data);
        let scm_credentials = ScmCredentials(ancillary_data_iter);
        AncillaryData::ScmCredentials(scm_credentials)
    }

    fn try_from_cmsghdr(cmsg: &'a libc::cmsghdr) -> Result<Self, AncillaryError> {
        unsafe {
            cfg_if::cfg_if! {
                if #[cfg(any(
                        target_os = "android",
                        all(target_os = "linux", target_env = "gnu"),
                        all(target_os = "linux", target_env = "uclibc"),
                   ))] {
                    let cmsg_len_zero = libc::CMSG_LEN(0) as libc::size_t;
                } else if #[cfg(any(
                              target_os = "dragonfly",
                              target_os = "emscripten",
                              target_os = "freebsd",
                              target_os = "macos",
                              all(target_os = "linux", target_env = "musl",),
                              target_os = "netbsd",
                              target_os = "openbsd",
                          ))] {
                    let cmsg_len_zero = libc::CMSG_LEN(0) as libc::socklen_t;
                }
            }
            let data_len = (*cmsg).cmsg_len - cmsg_len_zero;
            let data = libc::CMSG_DATA(cmsg).cast();
            let data = from_raw_parts(data, data_len as usize);

            match (*cmsg).cmsg_level {
                libc::SOL_SOCKET => match (*cmsg).cmsg_type {
                    libc::SCM_RIGHTS => Ok(AncillaryData::as_rights(data)),
                    #[cfg(any(target_os = "android", target_os = "linux",))]
                    libc::SCM_CREDENTIALS => Ok(AncillaryData::as_credentials(data)),
                    cmsg_type => Err(AncillaryError::Unknown {
                        cmsg_level: libc::SOL_SOCKET,
                        cmsg_type,
                    }),
                },
                cmsg_level => Err(AncillaryError::Unknown {
                    cmsg_level,
                    cmsg_type: (*cmsg).cmsg_type,
                }),
            }
        }
    }
}

/// This struct is used to iterate through the control messages.
pub struct Messages<'a> {
    buffer: &'a [u8],
    current: Option<&'a libc::cmsghdr>,
}

impl<'a> Iterator for Messages<'a> {
    type Item = Result<AncillaryData<'a>, AncillaryError>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            let mut msg: libc::msghdr = zeroed();
            msg.msg_control = self.buffer.as_ptr() as *mut _;
            cfg_if::cfg_if! {
                if #[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))] {
                    msg.msg_controllen = self.buffer.len() as libc::size_t;
                } else if #[cfg(any(
                              target_os = "dragonfly",
                              target_os = "emscripten",
                              target_os = "freebsd",
                              target_os = "macos",
                              all(target_os = "linux", target_env = "musl",),
                              target_os = "netbsd",
                              target_os = "openbsd",
                          ))] {
                    msg.msg_controllen = self.buffer.len() as libc::socklen_t;
                }
            }

            let cmsg = if let Some(current) = self.current {
                libc::CMSG_NXTHDR(&msg, current)
            } else {
                libc::CMSG_FIRSTHDR(&msg)
            };

            let cmsg = cmsg.as_ref()?;

            // Most operating systems, but not Linux or emscripten, return the previous pointer
            // when its length is zero. Therefore, check if the previous pointer is the same as
            // the current one.
            if let Some(current) = self.current {
                if eq(current, cmsg) {
                    return None;
                }
            }

            self.current = Some(cmsg);
            let ancillary_result = AncillaryData::try_from_cmsghdr(cmsg);
            Some(ancillary_result)
        }
    }
}

/// A Unix socket Ancillary data struct.
///
/// # Example
/// ```no_run
/// use privsep::net::{SocketAncillary, AncillaryData, StdUnixStreamExt};
/// use std::io::IoSliceMut;
/// use std::os::unix::net::UnixStream;
///
/// fn main() -> std::io::Result<()> {
///     let sock = UnixStream::connect("/tmp/sock")?;
///
///     let mut fds = [0; 8];
///     let mut ancillary_buffer = [0; 128];
///     let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);
///
///     let mut buf = [1; 8];
///     let mut bufs = &mut [IoSliceMut::new(&mut buf[..])][..];
///     sock.recv_vectored_with_ancillary(bufs, &mut ancillary)?;
///
///     for ancillary_result in ancillary.messages().flatten() {
///         if let AncillaryData::ScmRights(scm_rights) = ancillary_result {
///             for fd in scm_rights {
///                 println!("receive file descriptor: {}", fd);
///             }
///         }
///     }
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct SocketAncillary<'a> {
    buffer: &'a mut [u8],
    length: usize,
    truncated: bool,
}

impl<'a> SocketAncillary<'a> {
    /// Create an ancillary data with the given buffer.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # #![allow(unused_mut)]
    /// use privsep::net::SocketAncillary;
    /// let mut ancillary_buffer = [0; 128];
    /// let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);
    /// ```
    pub fn new(buffer: &'a mut [u8]) -> Self {
        SocketAncillary {
            buffer,
            length: 0,
            truncated: false,
        }
    }

    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the number of used bytes.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Checks if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Returns the iterator of the control messages.
    pub fn messages(&self) -> Messages<'_> {
        Messages {
            buffer: &self.buffer[..self.length],
            current: None,
        }
    }

    /// Is `true` if during a recv operation the ancillary was truncated.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use privsep::net::{SocketAncillary, AncillaryData, StdUnixStreamExt};
    /// use std::io::IoSliceMut;
    /// use std::os::unix::net::UnixStream;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let sock = UnixStream::connect("/tmp/sock")?;
    ///
    ///     let mut ancillary_buffer = [0; 128];
    ///     let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);
    ///
    ///     let mut buf = [1; 8];
    ///     let mut bufs = &mut [IoSliceMut::new(&mut buf[..])][..];
    ///     sock.recv_vectored_with_ancillary(bufs, &mut ancillary)?;
    ///
    ///     println!("Is truncated: {}", ancillary.truncated());
    ///     Ok(())
    /// }
    /// ```
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Add file descriptors to the ancillary data.
    ///
    /// The function returns `true` if there was enough space in the buffer.
    /// If there was not enough space then no file descriptors was appended.
    /// Technically, that means this operation adds a control message with the level `SOL_SOCKET`
    /// and type `SCM_RIGHTS`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use privsep::net::{SocketAncillary, AncillaryData, StdUnixStreamExt};
    /// use std::io::IoSlice;
    /// use std::os::unix::net::UnixStream;
    /// use std::os::unix::io::AsRawFd;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let sock = UnixStream::connect("/tmp/sock")?;
    ///
    ///     let mut ancillary_buffer = [0; 128];
    ///     let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);
    ///     ancillary.add_fds(&[sock.as_raw_fd()][..]);
    ///
    ///     let mut buf = [1; 8];
    ///     let mut bufs = &mut [IoSlice::new(&mut buf[..])][..];
    ///     sock.send_vectored_with_ancillary(bufs, &mut ancillary)?;
    ///     Ok(())
    /// }
    /// ```
    pub fn add_fds(&mut self, fds: &[RawFd]) -> bool {
        self.truncated = false;
        add_to_ancillary_data(
            &mut self.buffer,
            &mut self.length,
            fds,
            libc::SOL_SOCKET,
            libc::SCM_RIGHTS,
        )
    }

    /// Add credentials to the ancillary data.
    ///
    /// The function returns `true` if there was enough space in the buffer.
    /// If there was not enough space then no credentials was appended.
    /// Technically, that means this operation adds a control message with the level `SOL_SOCKET`
    /// and type `SCM_CREDENTIALS` or `SCM_CREDS`.
    ///
    #[cfg(any(doc, target_os = "android", target_os = "linux",))]
    pub fn add_creds(&mut self, creds: &[SocketCred]) -> bool {
        self.truncated = false;
        add_to_ancillary_data(
            &mut self.buffer,
            &mut self.length,
            creds,
            libc::SOL_SOCKET,
            libc::SCM_CREDENTIALS,
        )
    }

    /// Clears the ancillary data, removing all values.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use privsep::net::{SocketAncillary, AncillaryData, StdUnixStreamExt};
    /// use std::io::IoSliceMut;
    /// use std::os::unix::net::UnixStream;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let sock = UnixStream::connect("/tmp/sock")?;
    ///
    ///     let mut fds1 = [0; 8];
    ///     let mut fds2 = [0; 8];
    ///     let mut ancillary_buffer = [0; 128];
    ///     let mut ancillary = SocketAncillary::new(&mut ancillary_buffer[..]);
    ///
    ///     let mut buf = [1; 8];
    ///     let mut bufs = &mut [IoSliceMut::new(&mut buf[..])][..];
    ///
    ///     sock.recv_vectored_with_ancillary(bufs, &mut ancillary)?;
    ///     for ancillary_result in ancillary.messages().flatten() {
    ///         if let AncillaryData::ScmRights(scm_rights) = ancillary_result {
    ///             for fd in scm_rights {
    ///                 println!("receive file descriptor: {}", fd);
    ///             }
    ///         }
    ///     }
    ///
    ///     ancillary.clear();
    ///
    ///     sock.recv_vectored_with_ancillary(bufs, &mut ancillary)?;
    ///     for ancillary_result in ancillary.messages().flatten() {
    ///         if let AncillaryData::ScmRights(scm_rights) = ancillary_result {
    ///             for fd in scm_rights {
    ///                 println!("receive file descriptor: {}", fd);
    ///             }
    ///         }
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn clear(&mut self) {
        self.length = 0;
        self.truncated = false;
    }
}
