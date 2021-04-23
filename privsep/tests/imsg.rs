use privsep::{imsg, net::Fd};
use serde_derive::{Deserialize, Serialize};
use std::{io, net::TcpListener, os::unix::io::IntoRawFd, time::Duration};
use tokio::time::interval;

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    id: usize,
    name: String,
}

#[tokio::test(flavor = "multi_thread")]
async fn test_imsg() -> Result<(), io::Error> {
    unix_channel().await
}

async fn unix_channel() -> Result<(), std::io::Error> {
    let (sender, receiver) = imsg::Handler::pair()?;
    let mut count = 3;

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_millis(100));

        let fd = TcpListener::bind("127.0.0.1:1234")
            .ok()
            .map(|stream| stream.into_raw_fd())
            .map(Fd::from);

        for id in 1..=count {
            interval.tick().await;

            let message = Message {
                id,
                name: "test".to_string(),
            };

            if let Err(err) = sender
                .send_message(imsg::Message::min(), fd.as_ref(), &message)
                .await
            {
                eprintln!("Failed to send message: {}", err);
            }
        }
    });

    let res = loop {
        match { receiver.recv_message::<Message>().await } {
            Ok(None) => break Ok(()),
            Ok(Some((imsg, fd, message))) => {
                count -= 1;
                println!(
                    "Received message: {:?}, fd: {:?}, data: {:?}",
                    imsg, fd, message
                );
            }
            Err(err) => {
                eprintln!("Failed to receive messafe: {}", err);
                break Err(err);
            }
        }
    };

    assert!(count == 0, "did not receive expected messages");

    res
}
