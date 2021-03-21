use futures::{channel::mpsc, SinkExt, StreamExt};
use privsep::{imsg, net::Fd};
use serde_derive::{Deserialize, Serialize};
use std::{io, net::TcpListener, os::unix::io::IntoRawFd, time::Duration};
use tokio::time::interval;

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    id: usize,
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), io::Error> {
    unix_channel().await?;
    main_channel().await
}

async fn unix_channel() -> Result<(), std::io::Error> {
    let (sender, receiver) = imsg::Handler::pair()?;

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_millis(1));

        let fd = TcpListener::bind("127.0.0.1:1234")
            .ok()
            .map(|stream| stream.into_raw_fd())
            .map(Fd::from);

        for id in 1..=3 {
            interval.tick().await;

            let message = Message {
                id,
                name: "foo".to_string(),
            };

            if let Err(err) = sender
                .send_message(1u32.into(), fd.as_ref(), &message)
                .await
            {
                eprintln!("Failed to send ping: {}", err);
            }
        }
    });

    loop {
        match { receiver.recv_message::<Message>().await } {
            Ok(None) => break Ok(()),
            Ok(Some((imsg, fd, message))) => {
                println!(
                    "Received ping: {:?}, fd: {:?}, data: {:?}",
                    imsg, fd, message
                );
            }
            Err(err) => {
                eprintln!("Failed to receive ping: {}", err);
                break Err(err);
            }
        }
    }
}

async fn main_channel() -> Result<(), std::io::Error> {
    let (mut sender, mut receiver) = mpsc::unbounded::<usize>();

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(1));

        for i in 1..=3 {
            interval.tick().await;

            if let Err(err) = sender.send(i).await {
                eprintln!("Failed to send ping: {}", err);
            }
        }
    });

    while let Some(ping) = receiver.next().await {
        println!("Received ping: {}", ping);
    }

    Ok(())
}
