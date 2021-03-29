use crate::{error::Error, imsg::Handler};
use arrayvec::ArrayVec;
use derive_more::{Deref, Display, From};
use nix::{
    fcntl::{fcntl, FcntlArg, FdFlag},
    unistd::{execve, fork, getpid, getuid, ForkResult, Pid},
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

/// Internal env variable that is passed between processes.
pub const PRIVSEP_FD: &str = "PRIVSEP_FD";

/// This should typically be overwritten by the service implementing it.
pub const PRIVSEP_USERNAME: &str = "nobody";

/// General options for the privsep setup
#[derive(Debug)]
pub struct Options {
    /// This stop requiring root and disables privdrop.
    pub disable_privdrop: bool,
    /// The default privdrop username, if enabled.
    pub username: Username,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            disable_privdrop: false,
            username: Default::default(),
        }
    }
}

#[derive(Debug, Deref, Display, From)]
#[from(forward)]
pub struct Username(Cow<'static, str>);

impl Default for Username {
    fn default() -> Self {
        PRIVSEP_USERNAME.into()
    }
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
        if !options.disable_privdrop && !getuid().is_root() {
            return Err(Error::PermissionDenied);
        }

        let program = env::current_exe()?;
        let mut children = Children::default();

        for proc in &processes {
            let (handler, remote) = Handler::pair()?;

            let pid = match unsafe { fork() }? {
                ForkResult::Parent { child, .. } => child,
                ForkResult::Child => {
                    let fd = remote.as_raw_fd();
                    disable_cloexec(fd)?;

                    let name = path_to_cstr(&program);
                    execve(
                        &name,
                        &[&CString::new(proc.name).unwrap()],
                        &[&CString::new(format!("{}={}", PRIVSEP_FD, fd.to_string())).unwrap()],
                    )?;
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
    pub async fn new(name: &'static str, _options: &Options) -> Result<Self, Error> {
        let fd: RawFd = env::var(PRIVSEP_FD)?.parse()?;
        env::remove_var(PRIVSEP_FD);

        let parent = Handler::from_raw_fd(fd)?;

        Ok(Self {
            name,
            pid: getpid(),
            parent,
        })
    }
}

fn disable_cloexec(fd: RawFd) -> Result<(), Error> {
    let mut flags = FdFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFD)?);
    flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(fd, FcntlArg::F_SETFD(flags))?;
    Ok(())
}

fn path_to_cstr(path: &Path) -> CString {
    let ospath = path.as_os_str().as_bytes().to_vec();
    unsafe { CString::from_vec_unchecked(ospath) }
}
