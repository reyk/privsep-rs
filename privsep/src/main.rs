use futures::{channel::mpsc, SinkExt, StreamExt};
use privsep::net::stream::UnixStream;
use serde_derive::{Deserialize, Serialize};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::interval,
};

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    id: usize,
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), io::Error> {
    main_channel().await?;
    unix_channel().await
}

async fn unix_channel() -> Result<(), std::io::Error> {
    let (mut sender, mut receiver) = UnixStream::pair()?;

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(1));

        for id in 1..=3 {
            interval.tick().await;

            let message = Message {
                id,
                name: "foo".to_string(),
            };
            match bincode::serialize(&message) {
                Ok(buf) => {
                    if let Err(err) = sender.write_all(&buf).await {
                        eprintln!("Failed to send ping: {}", err);
                    }
                }
                Err(err) => eprintln!("Failed to deserialize ping: {}", err),
            }
        }
    });

    let mut buf = [0u8; 0xffff];
    loop {
        match receiver.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let message: Message = bincode::deserialize(&buf)
                    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
                println!("Received ping of {} bytes: {:?}", n, message)
            }
            Err(err) => {
                eprintln!("Failed to receive ping: {}", err);
                return Err(err);
            }
        }
    }

    Ok(())
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
