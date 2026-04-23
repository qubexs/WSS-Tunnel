use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[tokio::main]
async fn main() -> Result<()> {
    let relay_url = env::var("WSS_RELAY_URL")
        .expect("WSS_RELAY_URL not set");
    let domain = env::var("WSS_DOMAIN")
        .expect("WSS_DOMAIN not set");

    println!("Connecting to relay: {}", relay_url);
    println!("Tunneling domain: {}", domain);

    let (ws_stream, _) = connect_async(&relay_url).await
        .map_err(|e| anyhow::anyhow!("Failed to connect: {}", e))?;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    ws_sender.send(Message::Text(domain.into())).await
        .map_err(|e| anyhow::anyhow!("Failed to send domain: {}", e))?;

    let response = ws_receiver.next().await
        .ok_or_else(|| anyhow::anyhow!("No response from server"))??;

    if let Message::Text(text) = response {
        if text.as_str() == "DENIED" {
            anyhow::bail!("Access denied by relay server");
        }
    }

    println!("Connected! Tunnel open.");

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    let stdin_to_ws = async {
        let mut buf = [0u8; 4096];
        loop {
            let n = stdin.read(&mut buf).await.map_err(anyhow::Error::from)?;
            if n == 0 { break; }
            ws_sender.send(Message::Binary(buf[..n].to_vec())).await.map_err(anyhow::Error::from)?;
        }
        Ok::<_, anyhow::Error>(())
    };

    let ws_to_stdout = async {
        while let Some(msg) = ws_receiver.next().await {
            let msg = msg.map_err(anyhow::Error::from)?;
            match msg {
                Message::Binary(data) => {
                    stdout.write_all(&data).await.map_err(anyhow::Error::from)?;
                }
                Message::Text(text) => {
                    stdout.write_all(text.as_bytes()).await.map_err(anyhow::Error::from)?;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    };

    tokio::try_join!(stdin_to_ws, ws_to_stdout)?;

    Ok(())
}