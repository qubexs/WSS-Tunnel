use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

#[derive(Clone, Debug)]
struct DomainRoute {
    domain: String,
    target: String,
}

#[derive(Clone)]
struct AppState {
    routes: Arc<Vec<DomainRoute>>,
    token: String,
    sessions: Arc<RwLock<HashMap<String, String>>>,
    ip_connections: Arc<RwLock<HashMap<String, Vec<Instant>>>>,
    max_connections_per_ip: usize,
}

#[derive(Debug, Deserialize)]
struct AuthMessage {
    #[serde(rename = "type")]
    msg_type: String,
    token: String,
    domain: Option<String>,
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

fn validate_10_network(target: &str) -> Result<bool> {
    let (host, _) = target
        .rsplit_once(':')
        .unwrap_or((target, ""));

    let ip: IpAddr = host.parse()
        .map_err(|e| anyhow::anyhow!("Invalid IP: {}", e))?;

    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            Ok(octets[0] == 10)
        }
        IpAddr::V6(_) => Ok(false),
    }
}

const BUFFER_SIZE: usize = 65536;
const CONNECTION_WINDOW_SECS: u64 = 60;

async fn check_rate_limit(state: &AppState, ip: &str) -> Result<()> {
    let now = Instant::now();
    let window = Duration::from_secs(CONNECTION_WINDOW_SECS);

    {
        let mut connections = state.ip_connections.write().await;
        
        connections.retain(|_, times| {
            times.retain(|&t| t + window > now);
            !times.is_empty()
        });

        let count = connections.get(ip).map(|v| v.len()).unwrap_or(0);
        
        if count >= state.max_connections_per_ip {
            anyhow::bail!("Rate limit exceeded: {} connections from {} in last {}s", 
                count, ip, CONNECTION_WINDOW_SECS);
        }

        connections.entry(ip.to_string()).or_default().push(now);
    }

    Ok(())
}

async fn handle_connection(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    state: Arc<AppState>,
    client_ip: &str,
) -> Result<()> {
    let (mut ws_sender, mut ws_receiver) = ws.split();
    let mut session_id = String::new();

    let auth_msg = ws_receiver
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("No auth message received"))??;

    let auth: AuthMessage = match auth_msg {
        Message::Text(text) => {
            serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("Invalid auth format: {}", e))?
        }
        Message::Binary(data) => {
            let text = String::from_utf8_lossy(&data);
            serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("Invalid auth format: {}", e))?
        }
        _ => anyhow::bail!("Invalid first message"),
    };

    if auth.msg_type != "auth" {
        anyhow::bail!("First message must be auth");
    }

    if auth.token != state.token {
        ws_sender
            .send(Message::Text(r#"{"type":"error","message":"Invalid token"}"#.into()))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send error: {}", e))?;
        anyhow::bail!("Invalid token");
    }

    let domain = auth.domain.ok_or_else(|| anyhow::anyhow!("No domain in auth message"))?;
    let target = resolve_target(&state.routes, &domain);

    match target {
        Some(target_addr) => {
            let is_valid = validate_10_network(&target_addr)?;
            
            if is_valid {
                session_id = Uuid::new_v4().to_string();

                {
                    let mut sessions = state.sessions.write().await;
                    sessions.insert(session_id.clone(), domain.clone());
                }

                let ok_msg = serde_json::json!({
                    "type": "ok",
                    "session_id": session_id
                });
                ws_sender
                    .send(Message::Text(ok_msg.to_string().into()))
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to send OK: {}", e))?;

                println!("[{}] [{}] [{}] Connected", session_id, domain, client_ip);

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
                    let mut buf = vec![0u8; BUFFER_SIZE];
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

                let result = tokio::try_join!(ws_to_tcp, tcp_to_ws);

                {
                    let mut sessions = state.sessions.write().await;
                    sessions.remove(&session_id);
                }

                if let Err(e) = result {
                    eprintln!("[{}] [{}] Error: {}", session_id, domain, e);
                } else {
                    println!("[{}] [{}] Disconnected", session_id, domain);
                }
            } else {
                ws_sender
                    .send(Message::Text(r#"{"type":"error","message":"Denied: must be 10.x.x.x"}"#.into()))
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to send DENIED: {}", e))?;
                anyhow::bail!("Target must be in 10.x.x.x network");
            }
        }
        None => {
            ws_sender
                .send(Message::Text(r#"{"type":"error","message":"Domain not found"}"#.into()))
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
    let token = env::var("WSS_TOKEN").expect("WSS_TOKEN not set");
    let max_connections: usize = env::var("WSS_MAX_CONNECTIONS")
        .unwrap_or_else(|_| "100".to_string())
        .parse()
        .unwrap_or(100);

    let routes = Arc::new(parse_routes(&routes_str));
    let state = Arc::new(AppState {
        routes,
        token: token.clone(),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        ip_connections: Arc::new(RwLock::new(HashMap::new())),
        max_connections_per_ip: max_connections,
    });

    let token_shown = format!("{}***", &token[..4.min(token.len())]);
    println!("Token configured: {}", token_shown);
    println!("Max connections per IP: {}", max_connections);
    println!("Routes: {:?}", state.routes);

    let addr: SocketAddr = listen.parse()?;
    let listener = TcpListener::bind(addr).await?;

    println!("WebSocket relay server listening on {}", addr);
    println!("TLS should be handled by nginx/traefik (port 443 -> 8080)");

    loop {
        let (stream, addr) = listener.accept().await?;
        let client_ip = addr.ip().to_string();
        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = check_rate_limit(&state, &client_ip).await {
                eprintln!("[{}] Rate limit exceeded", client_ip);
                return;
            }

            match accept_async(stream).await {
                Ok(ws) => {
                    if let Err(e) = handle_connection(ws, state, &client_ip).await {
                        eprintln!("Connection error {}: {}", client_ip, e);
                    }
                }
                Err(e) => {
                    eprintln!("Accept error {}: {}", client_ip, e);
                }
            }
        });
    }
}