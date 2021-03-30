use crate::{error::Error, imsg::Handler};
use arrayvec::ArrayVec;
use close_fds::close_open_fds;
use derive_more::{Deref, Display, From};
use nix::{
    fcntl::{fcntl, FcntlArg, FdFlag},
    unistd::{self, chdir, chroot, dup2, execv, fork, getpid, getuid, ForkResult, Pid, User},
};
use std::{
    borrow::Cow,
    env,
    ffi::CString,
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
    },
    path::Path,
};

/// Internal file descriptor that is passed between processes.
pub const PRIVSEP_FD: RawFd = 3;

/// General options for the privsep setup
#[derive(Debug, Default)]
pub struct Options {
    /// This stop requiring root and disables privdrop.
    pub disable_privdrop: bool,
    /// The default privdrop username, if enabled.
    pub username: Cow<'static, str>,
}

#[derive(Debug, From)]
pub struct Process {
    pub name: &'static str,
}

pub type Processes<const N: usize> = [Process; N];

#[derive(Debug, Deref)]
pub struct ChildProcess {
    #[deref]
    pub handler: Handler,
    pub pid: Pid,
}

pub type Children<const N: usize> = ArrayVec<ChildProcess, N>;

#[derive(Debug, Display, Deref)]
#[display(fmt = "parent({})", "pid")]
pub struct Parent<const N: usize> {
    pub pid: Pid,
    pub processes: Processes<N>,
    #[deref]
    pub children: Children<N>,
}

impl<const N: usize> Parent<N> {
    pub async fn new(processes: Processes<N>, options: &Options) -> Result<Parent<N>, Error> {
        if options.disable_privdrop && !getuid().is_root() {
            return Err(Error::PermissionDenied);
        }

        let program = env::current_exe()?;
        let mut children = Children::default();

        for proc in &processes {
            let (handler, remote) = Handler::pair()?;

            let pid = match unsafe { fork() }? {
                ForkResult::Parent { child, .. } => child,
                ForkResult::Child => {
                    let name = path_to_cstr(&program);

                    let fd = dup2(remote.as_raw_fd(), PRIVSEP_FD).unwrap();
                    set_cloexec(fd, false)?;

                    // TODO: open /dev/null and dup2 stdin/stdout/stderr.

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

                    execv(&name, &[&CString::new(proc.name).unwrap()])?;
                    return Err(Error::PermissionDenied);
                }
            };

            children.push(ChildProcess { handler, pid })
        }

        Ok(Self {
            pid: getpid(),
            processes,
            children,
        })
    }
}

#[derive(Debug, Deref, Display)]
#[display(fmt = "{}({})", "name, pid")]
pub struct Child {
    pub name: &'static str,
    pub pid: Pid,
    #[deref]
    pub parent: Handler,
}

impl Child {
    pub async fn new(name: &'static str, options: &Options) -> Result<Self, Error> {
        set_cloexec(PRIVSEP_FD, true)?;
        let parent = Handler::from_raw_fd(PRIVSEP_FD)?;

        if !options.disable_privdrop {
            // Get the privdrop user.
            let user = User::from_name(&options.username)?
                .ok_or_else(|| Error::UserNotFound(options.username.clone()))?;

            // chroot and change the working directory.
            chroot(&user.dir).map_err(|err| Error::Privdrop("chroot", err.into()))?;
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

        Ok(Self {
            name,
            pid: getpid(),
            parent,
        })
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
