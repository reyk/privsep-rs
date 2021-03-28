pub use privsep::{
    process::{Child, Main},
    Error,
};
use privsep_derive::Privsep;

/// Privsep processes.
#[derive(Debug, Privsep)]
pub enum Privsep {
    Child,
}

/// Privileged parent process.
mod parent {
    use crate::{Error, Privsep};
    use nix::sys::wait::{waitpid, WaitStatus};
    use privsep::{net::Fd, process::Parent};
    use std::{net::TcpListener, os::unix::io::IntoRawFd, process, time::Duration};
    use tokio::{
        signal::unix::{signal, SignalKind},
        time::sleep,
    };

    pub async fn main<const N: usize>(parent: Parent<N>) -> Result<(), Error> {
        println!("Hello, parent {}!", parent);

        let fd = TcpListener::bind("127.0.0.1:80")
            .ok()
            .map(|stream| stream.into_raw_fd())
            .map(Fd::from);

        parent[*Privsep::Child]
            .send_message(23u32.into(), fd.as_ref(), &())
            .await?;

        let mut sigchld = signal(SignalKind::child())?;

        loop {
            tokio::select! {
                _ = sigchld.recv() => {
                    match waitpid(None, None) {
                        Ok(WaitStatus::Exited(pid, status)) => {
                            println!("Child {} exited with status {}", pid, status);
                            process::exit(0);
                        }
                        status => {
                            println!("Child exited with error: {:?}", status);
                            process::exit(1);
                        }
                    }
                }
                message = parent[*Privsep::Child].recv_message::<()>() => {
                    println!("{}: received message {:?}", parent, message);
                    if let Some((message, _, _)) = message? {
                        sleep(Duration::from_secs(1)).await;
                        parent[*Privsep::Child].send_message(message, fd.as_ref(), &()).await?;
                    }
                }
            }
        }
    }
}

/// Unprivileged child process.
mod child {
    use crate::Error;
    use privsep::process::Child;
    use std::{sync::Arc, time::Duration};
    use tokio::time::{interval, sleep};

    pub async fn main(child: Child) -> Result<(), Error> {
        let child = Arc::new(child);

        println!("Hello, child {}!", child);

        // Run parent handler as background task
        tokio::spawn({
            let child = child.clone();
            async move {
                loop {
                    if let Ok(message) = child.recv_message::<()>().await {
                        println!("{}: received message {:?}", child, message);
                        if let Some((message, _, _)) = message {
                            sleep(Duration::from_secs(1)).await;
                            if let Err(err) = child.send_message(message, None, &()).await {
                                eprintln!("failed to send message: {}", err);
                            }
                        }
                    }
                }
            }
        });

        // other client stuff here...
        let mut interval = interval(Duration::from_secs(3));
        loop {
            interval.tick().await;
            println!("{}: tick", child);
        }
    }
}

/// Shared entry point.
async fn start() -> Result<(), Error> {
    match Main::new(Privsep::as_array())? {
        Main::Child(child @ Child { name: "child", .. }) => child::main(child).await,
        Main::Parent(parent) => parent::main(parent).await,
        _ => Err("invalid process".into()),
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = start().await {
        eprintln!("Error: {}", err);
    }
}
