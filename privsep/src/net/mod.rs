//! Networking for `imsg` handling and file descriptor passing.

mod ancillary;
mod fd;
mod stream;

pub use ancillary::{AncillaryData, SocketAncillary};
pub use fd::Fd;
pub use stream::{StdUnixStreamExt, UnixStream, UnixStreamExt};
