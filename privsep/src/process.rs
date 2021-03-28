use crate::{error::Error, imsg::Handler};
use arrayvec::ArrayVec;
use derive_more::{Deref, Display, From};
use nix::{
    fcntl::{fcntl, FcntlArg, FdFlag},
    unistd::{execve, fork, getpid, getuid, ForkResult, Pid},
};
use std::{
    env,
    ffi::CString,
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
    },
    path::Path,
};

pub const PRIVSEP_FD: &str = "PRIVSEP_FD";

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

#[derive(Debug, From)]
pub enum Main<const N: usize> {
    Parent(Parent<N>),
    Child(Child),
}

impl<const N: usize> Main<N> {
    // TODO: split new and change run
    pub fn new(processes: Processes<N>) -> Result<Self, Error> {
        let name = env::args().next().unwrap_or_default();

        for process in &processes {
            if process.name == name {
                return Ok(Child::new(process.name)?.into());
            }
        }

        Parent::new(processes).map(Into::into)
    }
}

#[derive(Debug, Display, Deref)]
#[display(fmt = "parent({})", "pid")]
pub struct Parent<const N: usize> {
    pub pid: Pid,
    pub processes: Processes<N>,
    #[deref]
    pub children: Children<N>,
}

impl<const N: usize> Parent<N> {
    pub fn new(processes: Processes<N>) -> Result<Parent<N>, Error> {
        if !getuid().is_root() {
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
    pub fn new(name: &'static str) -> Result<Self, Error> {
        let fd: RawFd = env::var(PRIVSEP_FD)?.parse()?;
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
