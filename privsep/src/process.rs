//! Configuration and setup of privilege-separated processes.

use crate::{
    error::Error,
    imsg::{Handler, Message},
};
use arrayvec::ArrayVec;
use close_fds::close_open_fds;
use derive_more::{AsRef, Deref, Display, From};
use nix::{
    fcntl::{fcntl, open, FcntlArg, FdFlag, OFlag},
    sys::{
        signal::{signal, SigHandler, Signal},
        stat::Mode,
    },
    unistd::{
        self, chdir, chroot, close, dup2, execve, fork, geteuid, setsid, ForkResult, Pid, User,
    },
};
use std::{
    borrow::Cow,
    collections::HashSet,
    env,
    ffi::CString,
    ops,
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
    },
    path::Path,
};

/// Internal file descriptor that is passed between processes.
pub const PRIVSEP_FD: RawFd = libc::STDERR_FILENO + 1;

/// Reserved name for the parent process.
pub const PARENT: &str = "parent";

/// Runtime-configurable options for the privsep setup.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Whether to run the program in foreground.
    pub foreground: bool,
    /// The log_level if RUST_LOG is not set.
    pub log_level: Option<String>,
}

#[cfg(feature = "log")]
impl From<&Config> for privsep_log::Config {
    fn from(config: &Config) -> Self {
        Self {
            foreground: config.foreground,
            filter: config.log_level.clone(),
        }
    }
}

/// General options for the privsep setup.
#[derive(Debug, Default, From)]
pub struct Options {
    /// This stop requiring root and disables privdrop.
    pub disable_privdrop: bool,
    /// The default privdrop username, if enabled.
    pub username: Cow<'static, str>,
    /// The runtime configuration.
    pub config: Config,
}

/// Child process startup definition.
#[derive(AsRef, Debug, From)]
pub struct Process {
    /// The process name.
    #[as_ref]
    pub name: &'static str,
    /// Connect this process.
    pub connect: bool,
}

/// The list of child process definitions.
pub type Processes<const N: usize> = [Process; N];

/// A child process from the parent point of view.
#[derive(Debug, AsRef)]
pub struct Peer {
    /// The process name.
    #[as_ref]
    pub name: &'static str,
    /// IPC channel to the child process.
    pub handler: Option<Handler>,
    /// Process PID.
    pub pid: Pid,
}

impl Default for Peer {
    fn default() -> Self {
        Self {
            name: "",
            handler: None,
            pid: Pid::parent(),
        }
    }
}

impl ops::Deref for Peer {
    type Target = Handler;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // This panics when the handler is None which should never
        // happen as it violates the configured privsep channels.
        self.handler
            .as_ref()
            .unwrap_or_else(|| panic!("unconfigured privsep channel: {}", self.name))
    }
}

/// The list of child processes.
pub type Peers<const N: usize> = ArrayVec<Peer, N>;

/// The privileged parent.
#[derive(Debug, Display, Deref)]
#[display(fmt = "{}({})", "crate::process::PARENT", "pid")]
pub struct Parent<const N: usize> {
    /// Process PID.
    pub pid: Pid,
    /// Child processes.
    #[deref]
    pub children: Peers<N>,
}

impl<const N: usize> Parent<N> {
    /// Creates a new parent and forks the children.
    pub async fn new(processes: Processes<N>, options: &Options) -> Result<Parent<N>, Error> {
        if !options.disable_privdrop && !geteuid().is_root() {
            return Err(Error::PermissionDenied);
        }

        if processes.first().map(|process| process.name) != Some(PARENT) {
            return Err(Error::MissingParent);
        }

        let program = env::current_exe()?;
        let mut children = Peers::default();

        for proc in &processes {
            if !proc.connect {
                children.push(Peer {
                    name: proc.name,
                    handler: None,
                    pid: Pid::this(),
                });
                continue;
            }
            let (handler, remote) = Handler::pair()?;

            let pid = match unsafe { fork() }? {
                ForkResult::Parent { child, .. } => child,
                ForkResult::Child => {
                    // Create a new session for the executed process.
                    new_session(options.config.foreground, true)?;

                    let fd = dup2(remote.as_raw_fd(), PRIVSEP_FD)?;
                    set_cloexec(fd, false)?;

                    // TODO: we could eventually implement `closefrom`
                    // ourselves based on OpenSSH's `bsd-closefrom.c`.
                    //
                    // Rust sets most file descriptors to
                    // close-on-exec but we make sure that any
                    // additional file descriptors are closed.  This
                    // is using the `close_fds` crate because a
                    // BSD-like `closefrom` is not part of `nix`.
                    unsafe {
                        close_open_fds(PRIVSEP_FD + 1, &[]);
                    }

                    let name = path_to_cstr(&program);
                    let args = [
                        &CString::new(proc.name).unwrap(),
                        &CString::new(if options.config.foreground { "-d" } else { "" }).unwrap(),
                    ];
                    let env = [&CString::new(format!(
                        "RUST_LOG={}",
                        env::var("RUST_LOG")
                            .ok()
                            .as_deref()
                            .or_else(|| options.config.log_level.as_deref())
                            .unwrap_or_default()
                    ))
                    .unwrap()];

                    execve(&name, &args, &env)?;

                    return Err(Error::PermissionDenied);
                }
            };

            children.push(Peer {
                name: proc.name,
                handler: Some(handler),
                pid,
            })
        }

        assert_eq!(children.len(), N, "child processes");

        // Closing the imsg pipes will terminate the program.
        unsafe { signal(Signal::SIGPIPE, SigHandler::SigIgn) }?;

        Ok(Self {
            pid: Pid::this(),
            children,
        })
    }

    pub async fn connect(self, processes: [Processes<N>; N]) -> Result<Self, Error> {
        // Filter for bi-directional child-child connections.
        let pairs = processes
            .iter()
            .enumerate()
            .skip(1)
            .flat_map(|(a, outer)| {
                outer
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter_map(move |(b, inner)| {
                        if !inner.connect || a == b {
                            None
                        } else if a < b {
                            Some((a, b))
                        } else {
                            Some((b, a))
                        }
                    })
            })
            .collect::<HashSet<_>>();

        for (a, b) in pairs {
            let (left, right) = Handler::socketpair()?;

            self[a]
                .send_message_internal(Message::connect(b), Some(&left), &())
                .await?;
            self[b]
                .send_message_internal(Message::connect(a), Some(&right), &())
                .await?;
        }

        Ok(self)
    }
}

/// A child process.
#[derive(AsRef, Debug, Deref, Display)]
#[display(fmt = "{}({})", "name, pid")]
pub struct Child<const N: usize> {
    /// Process name.
    #[as_ref]
    pub name: &'static str,
    /// Process PID.
    pub pid: Pid,
    /// Process' parenr handler.
    #[deref]
    pub peers: Peers<N>,
}

impl<const N: usize> Child<N> {
    /// Creates a new child and drops privileges.
    pub async fn new<const M: usize>(
        processes: Processes<M>,
        name: &'static str,
        options: &Options,
    ) -> Result<Self, Error> {
        // TODO: replace this with complex const generic constraints, once stable.
        assert!(M <= N);

        set_cloexec(PRIVSEP_FD, true)?;

        let mut peers = Peers::default();
        peers.push(Peer {
            name: processes[0].name,
            handler: Some(Handler::from_raw_fd(PRIVSEP_FD)?),
            ..Peer::default()
        });
        for process in processes.iter().skip(1) {
            peers.push(Peer {
                name: process.name,
                ..Peer::default()
            });
        }

        if !options.disable_privdrop {
            // Get the privdrop user.
            let user = User::from_name(&options.username)?
                .ok_or_else(|| Error::UserNotFound(options.username.clone()))?;

            // chroot and change the working directory.
            let dir = if user.dir.is_dir() {
                user.dir.as_path()
            } else {
                Path::new("/var/empty")
            };
            chroot(dir).map_err(|err| Error::Privdrop("chroot", err.into()))?;
            chdir("/").map_err(|err| Error::Privdrop("chdir", err.into()))?;

            // Set the supplementary groups.
            #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "redox")))]
            unistd::setgroups(&[user.gid])
                .map_err(|err| Error::Privdrop("setgroups", err.into()))?;

            // Drop the privileges.
            cfg_if::cfg_if! {
                if #[cfg(any(target_os = "android", target_os = "freebsd",
                             target_os = "linux", target_os = "openbsd"))] {
                    unistd::setresgid(user.gid, user.gid, user.gid)
                        .map_err(|err| Error::Privdrop("setresgid", err.into()))?;
                    unistd::setresuid(user.uid, user.uid, user.uid)
                        .map_err(|err| Error::Privdrop("setresuid", err.into()))?;
                } else {
                    unistd::setegid(user.gid).map_err(|err| Error::Privdrop("setegid", err.into()))?;
                    unistd::setgid(user.gid).map_err(|err| Error::Privdrop("setgid", err.into()))?;
                    // seteuid before setuid fails on macOS (and AIX...)
                    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
                    unistd::seteuid(user.uid).map_err(|err| Error::Privdrop("seteuid", err.into()))?;
                    unistd::setuid(user.uid).map_err(|err| Error::Privdrop("setuid", err.into()))?;
                }
            }
        }

        // Closing the imsg pipes will terminate the program.
        unsafe { signal(Signal::SIGPIPE, SigHandler::SigIgn) }?;

        // Wait for imsg sockets to peer processes.
        let mut wait_connections = processes
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(id, proc)| proc.connect.then(|| id))
            .collect::<HashSet<_>>();

        while !wait_connections.is_empty() {
            match peers[0].recv_message().await? {
                Some((Message { id: 1, peer_id, .. }, Some(fd), ())) => {
                    let peer_id = peer_id as usize;
                    if !wait_connections.remove(&peer_id) {
                        panic!("Received invalid peer message, terminating");
                    }
                    fd.is_open()?;
                    println!("{} connect {}", name, peers[peer_id].name);
                    peers[peer_id].handler = Some(Handler::from_raw_fd(fd)?);
                }
                _ => panic!("Failed to get peer message, terminating"),
            }
        }

        Ok(Self {
            name,
            pid: Pid::this(),
            peers,
        })
    }

    /// Forcefully close all imsg handlers without dropping them.
    pub fn shutdown(&self) {
        self.peers
            .iter()
            .map(ops::Deref::deref)
            .for_each(Handler::shutdown);
    }
}

fn set_cloexec(fd: RawFd, add: bool) -> Result<(), Error> {
    let mut flags = FdFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFD)?);
    flags.set(FdFlag::FD_CLOEXEC, add);
    fcntl(fd, FcntlArg::F_SETFD(flags))?;
    Ok(())
}

fn path_to_cstr(path: &Path) -> CString {
    let ospath = path.as_os_str().as_bytes().to_vec();
    unsafe { CString::from_vec_unchecked(ospath) }
}

/// Portable wrapper of the daemon(3) function that got removed from macOS.
pub fn daemon(no_close: bool, no_chdir: bool) -> Result<(), Error> {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            match unsafe { fork() }? {
                ForkResult::Parent { .. } => unsafe { libc::_exit(0) },
                ForkResult::Child => new_session(no_close, no_chdir),
            }
        } else {
            unistd::daemon(no_close, no_chdir).map_err(Into::into)
        }
    }
}

fn new_session(no_close: bool, no_chdir: bool) -> Result<(), Error> {
    // Create a new session for the executed processes.
    if setsid()? != Pid::this() {
        return Err("Failed to create new session".into());
    }

    if !no_chdir {
        let _ = chdir("/");
    }

    // Daemons detach from terminal.
    if !no_close {
        // Ignore errors as it is done in OpenSSH's daemon.c compat code.
        if let Ok(fd) = open("/dev/null", OFlag::O_RDWR, Mode::empty()) {
            let _ = dup2(fd, libc::STDIN_FILENO);
            let _ = dup2(fd, libc::STDOUT_FILENO);
            let _ = dup2(fd, libc::STDERR_FILENO);
            if fd > libc::STDERR_FILENO {
                let _ = close(fd);
            }
        }
    }

    Ok(())
}
