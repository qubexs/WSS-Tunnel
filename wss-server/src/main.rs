use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug)]
struct DomainRoute {
    domain: String,
    target: String,
}

fn parse_routes(routes_str: &str) -> Vec<DomainRoute> {
    routes_str
        .split(',')
        .filter_map(|s| {
            let parts: Vec<&str> = s.split('=').collect();
            if parts.len() == 2 {
                Some(DomainRoute {
                    domain: parts[0].trim().to_string(),
                    target: parts[1].trim().to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn resolve_target(routes: &[DomainRoute], domain: &str) -> Option<String> {
    routes
        .iter()
        .find(|r| r.domain == domain)
        .map(|r| r.target.clone())
}

async fn handle_connection(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    routes: Arc<Vec<DomainRoute>>,
) -> Result<()> {
    let (mut ws_sender, mut ws_receiver) = ws.split();

    let domain_msg = ws_receiver
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("No domain received"))??;

    let domain = match domain_msg {
        Message::Text(text) => text.to_string(),
        Message::Binary(data) => String::from_utf8_lossy(&data).to_string(),
        _ => anyhow::bail!("Invalid first message"),
    };

    let target = resolve_target(&routes, &domain);

    match target {
        Some(target_addr) if target_addr.starts_with("10.") => {
            println!("Connecting to {}", target_addr);
            ws_sender
                .send(Message::Text("OK".into()))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send OK: {}", e))?;

            let mut outbound = tokio::net::TcpStream::connect(&target_addr)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", target_addr, e))?;

            let (mut ro, mut wo) = outbound.split();

            let ws_to_tcp = async {
                while let Some(msg) = ws_receiver.next().await {
                    let msg = msg.map_err(anyhow::Error::from)?;
                    if msg.is_binary() {
                        wo.write_all(&msg.into_data()).await.map_err(anyhow::Error::from)?;
                    } else if msg.is_text() {
                        wo.write_all(msg.to_text().unwrap().as_bytes()).await.map_err(anyhow::Error::from)?;
                    }
                }
                Ok::<_, anyhow::Error>(())
            };

            let tcp_to_ws = async {
                let mut buf = [0u8; 8192];
                loop {
                    let n = ro.read(&mut buf).await.map_err(anyhow::Error::from)?;
                    if n == 0 {
                        break;
                    }
                    ws_sender
                        .send(Message::Binary(buf[..n].to_vec()))
                        .await
                        .map_err(anyhow::Error::from)?;
                }
                Ok::<_, anyhow::Error>(())
            };

            tokio::try_join!(ws_to_tcp, tcp_to_ws)?;
        }
        Some(_) => {
            ws_sender
                .send(Message::Text("DENIED".into()))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send DENIED: {}", e))?;
            anyhow::bail!("Target must be 10.x.x.x");
        }
        None => {
            ws_sender
                .send(Message::Text("NOT_FOUND".into()))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send NOT_FOUND: {}", e))?;
            anyhow::bail!("Domain not found: {}", domain);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let listen = env::var("WSS_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let routes_str = env::var("WSS_ROUTES").expect("WSS_ROUTES not set");

    let routes = Arc::new(parse_routes(&routes_str));
    println!("Routes: {:?}", routes);

    let addr: SocketAddr = listen.parse()?;
    let listener = TcpListener::bind(addr).await?;

    println!("WebSocket relay server listening on {}", addr);
    println!("TLS should be handled by nginx/traefik (port 443 -> 8080)");

    loop {
        let (stream, addr) = listener.accept().await?;
        let routes = routes.clone();

        tokio::spawn(async move {
            match accept_async(stream).await {
                Ok(ws) => {
                    if let Err(e) = handle_connection(ws, routes).await {
                        eprintln!("Connection error {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    eprintln!("Accept error {}: {}", addr, e);
                }
            }
        });
    }
}