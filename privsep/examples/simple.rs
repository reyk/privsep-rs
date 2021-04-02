//! Simple example of a privileged service.

pub use privsep::{process::Child, Error};
use privsep_derive::Privsep;
use std::env;

/// Privsep processes.
#[derive(Debug, Privsep)]
#[username = "nobody"]
pub enum Privsep {
    /// An unprivileged child process that prints hello.
    Hello,
    /// A copy of the hello process.
    #[main_path = "hello::main"]
    Child,
}

/// Privileged parent process.
mod parent {
    use crate::{Error, Privsep};
    use nix::sys::wait::{waitpid, WaitStatus};
    use privsep::{net::Fd, process::Parent};
    use privsep_log::{info, warn};
    use std::{net::TcpListener, os::unix::io::IntoRawFd, process, time::Duration};
    use tokio::{
        signal::unix::{signal, SignalKind},
        time::sleep,
    };

    // main entrypoint of the parent process
    pub async fn main<const N: usize>(parent: Parent<N>) -> Result<(), Error> {
        let _guard = privsep_log::init(&parent.to_string(), true).unwrap();

        info!("Hello, parent!");

        let mut sigchld = signal(SignalKind::child())?;

        let fd = TcpListener::bind("127.0.0.1:80")
            .ok()
            .map(|stream| stream.into_raw_fd())
            .map(Fd::from);

        // Send a message to all children.
        for id in Privsep::PROCESS_IDS.iter() {
            parent[*id]
                .send_message(23u32.into(), fd.as_ref(), &())
                .await?;
        }

        loop {
            tokio::select! {
                _ = sigchld.recv() => {
                    match waitpid(None, None) {
                        Ok(WaitStatus::Exited(pid, status)) => {
                            warn!("Child {} exited with status {}", pid, status);
                            process::exit(0);
                        }
                        status => {
                            warn!("Child exited with error: {:?}", status);
                            process::exit(1);
                        }
                    }
                }
                message = parent[Privsep::CHILD_ID].recv_message::<()>() => {
                    info!(
                        "received message {:?}", message;
                        "source" => Privsep::Child.as_ref(),
                    );
                    if let Some((message, _, _)) = message? {
                        sleep(Duration::from_secs(1)).await;
                        parent[Privsep::CHILD_ID].send_message(message, fd.as_ref(), &()).await?;
                    }
                }
                message = parent[Privsep::HELLO_ID].recv_message::<()>() => {
                    info!(
                        "received message {:?}", message;
                        "source" => Privsep::Hello.as_ref(),
                    );
                    if let Some((message, _, _)) = message? {
                        sleep(Duration::from_secs(1)).await;
                        parent[Privsep::HELLO_ID].send_message(message, fd.as_ref(), &()).await?;
                    }
                }
            }
        }
    }
}

/// Unprivileged child process.
mod hello {
    use crate::Error;
    use privsep::process::Child;
    use privsep_log::{debug, info, warn};
    use std::{sync::Arc, time::Duration};
    use tokio::time::{interval, sleep};

    // main entrypoint to the child processes
    pub async fn main(child: Child) -> Result<(), Error> {
        let _guard = privsep_log::init(&child.to_string(), true).unwrap();

        let child = Arc::new(child);

        info!("Hello, child {}!", child);

        // Run parent handler as background task
        tokio::spawn({
            let child = child.clone();
            async move {
                loop {
                    if let Ok(message) = child.recv_message::<()>().await {
                        info!("received message {:?}", message);
                        if let Some((message, _, _)) = message {
                            sleep(Duration::from_secs(1)).await;
                            if let Err(err) = child.send_message(message, None, &()).await {
                                warn!("failed to send message: {}", err);
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
            debug!("tick");
        }
    }
}

#[tokio::main]
async fn main() {
    env::set_var(
        "RUST_LOG",
        env::var("RUST_LOG").unwrap_or("debug".to_string()),
    );

    if let Err(err) = Privsep::main().await {
        eprintln!("Error: {}", err);
    }
}
