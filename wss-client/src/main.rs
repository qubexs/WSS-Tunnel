use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde::Serialize;
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt, stdin, stdout};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Serialize)]
struct AuthMessage {
    #[serde(rename = "type")]
    msg_type: String,
    token: String,
    domain: String,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    #[serde(rename = "type")]
    msg_type: String,
    session_id: Option<String>,
    message: Option<String>,
}

const MAX_RETRIES: u32 = 0;
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 30000;

async fn connect_and_tunnel(
    relay_url: &str,
    domain: &str,
    token: &str,
) -> Result<String> {
    let (ws_stream, _) = connect_async(relay_url).await
        .map_err(|e| anyhow::anyhow!("Failed to connect: {}", e))?;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let auth = AuthMessage {
        msg_type: "auth".to_string(),
        token: token.to_string(),
        domain: domain.to_string(),
    };
    let auth_json = serde_json::to_string(&auth)
        .map_err(|e| anyhow::anyhow!("Failed to serialize auth: {}", e))?;

    ws_sender.send(Message::Text(auth_json.into())).await
        .map_err(|e| anyhow::anyhow!("Failed to send auth: {}", e))?;

    let response = ws_receiver.next().await
        .ok_or_else(|| anyhow::anyhow!("No response from server"))??;

    let resp_text = response.to_text().unwrap_or("");
    let auth_resp: AuthResponse = serde_json::from_str(resp_text)
        .map_err(|e| anyhow::anyhow!("Invalid response: {}", e))?;

    if auth_resp.msg_type == "error" {
        anyhow::bail!("Server error: {}", auth_resp.message.unwrap_or_default());
    }

    let session_id = auth_resp.session_id.unwrap_or_else(|| "unknown".to_string());
    println!("Tunnel open [{}].", session_id);

    let mut stdin = stdin();
    let mut stdout = stdout();

    const BUFFER_SIZE: usize = 65536;

    let stdin_to_ws = async {
        let mut buf = vec![0u8; BUFFER_SIZE];
        loop {
            let n = stdin.read(&mut buf).await.map_err(anyhow::Error::from)?;
            if n == 0 { break; }
            ws_sender.send(Message::Binary(buf[..n].to_vec())).await.map_err(anyhow::Error::from)?;
        }
        Ok::<_, anyhow::Error>(())
    };

    let ws_to_stdout = async {
        let mut buf = vec![0u8; BUFFER_SIZE];
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

    Ok(session_id)
}

#[tokio::main]
async fn main() -> Result<()> {
    let relay_url = env::var("WSS_RELAY_URL")
        .expect("WSS_RELAY_URL not set");
    let domain = env::var("WSS_DOMAIN")
        .expect("WSS_DOMAIN not set");
    let token = env::var("WSS_TOKEN")
        .expect("WSS_TOKEN not set");

    println!("Connecting to relay: {}", relay_url);
    println!("Tunneling domain: {}", domain);
    println!("Press Ctrl+C to exit");

    let mut retries = 0u32;
    let mut backoff_ms = INITIAL_BACKOFF_MS;

    loop {
        match connect_and_tunnel(&relay_url, &domain, &token).await {
            Ok(session_id) => {
                println!("[{}] Tunnel closed.", session_id);
                break;
            }
            Err(e) => {
                if MAX_RETRIES > 0 && retries >= MAX_RETRIES {
                    eprintln!("Max retries ({}) reached. Giving up.", MAX_RETRIES);
                    break;
                }

                retries += 1;
                eprintln!("Error: {}. Reconnecting in {}ms (attempt {})...", 
                    e, backoff_ms, retries);

                sleep(Duration::from_millis(backoff_ms)).await;

                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }

    Ok(())
}