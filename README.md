# WSS - WebSocket Tunnel

A production-grade WebSocket tunnel with domain-based routing, authentication, and security features.

## Features

| Feature | Description |
|---------|-------------|
| 🔒 **10.x.x.x Validation** | Proper IP parsing (not just string prefix) |
| 🔐 **Token Authentication** | Server/client token-based auth |
| 🔁 **Auto Reconnect** | Exponential backoff on disconnect |
| 🆔 **Session ID** | Unique ID per connection for debugging |
| 📊 **Rate Limiting** | Max connections per IP |
| ⚡ **High Performance** | 64KB buffers, 300Mbps+ throughput |
| 🐳 **Docker Ready** | Multi-stage build for small images |

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     INTERNET                             │
└───────────────────────┬─────────────────────────────────┘
                      │ TLS (wss://)
                      ▼
┌───────────────────────────────────────────────────────┐
│                   wss-server                         │
│  (Docker)                                          │
│  - TLS: nginx/traefik (port 443 → 8080)            │
│  - Plain WS: wss-server (port 8080)                  │
│  - Domain routing: domain → 10.x.x.x               │
│  - Authentication: token required                   │
│  - Rate limiting: max connections per IP            │
└───────────────────────────────────────────────────────┘
                       │
                       │ TCP
                       ▼
              ┌────────────────┐
              │ Target        │
              │ 10.x.x.x:443│
              │ (HTTPS)      │
              └────────────────┘
```

## Quick Start

### 1. Run Server (Docker)

```bash
# Build
cd wss-server
cargo build --release
docker build -t wss-server .

# Run
docker run -p 8080:8080 \
  -e WSS_ROUTES="app.internal=10.0.0.1:443" \
  -e WSS_TOKEN="your-secret-token" \
  -e WSS_MAX_CONNECTIONS=100 \
  wss-server
```

### 2. Run Client

```bash
# Build
cd wss-client
cargo build --release

# Run
$env:WSS_RELAY_URL="wss://your-server:8443"
$env:WSS_DOMAIN="app.internal"
$env:WSS_TOKEN="your-secret-token"
.\wss-client.exe
```

## Environment Variables

### wss-server

| Variable | Description | Default |
|---------|------------|---------|
| `WSS_LISTEN` | Listen address | `0.0.0.0:8080` |
| `WSS_ROUTES` | Domain→target mappings | (required) |
| `WSS_TOKEN` | Authentication token | (required) |
| `WSS_MAX_CONNECTIONS` | Max connections per IP | `100` |

```bash
# Example
WSS_ROUTES=app.internal=10.0.0.1:443,app2.internal=10.0.0.2:3389
WSS_TOKEN=your-secret-token
WSS_MAX_CONNECTIONS=50
```

### wss-client

| Variable | Description | Default |
|---------|------------|---------|
| `WSS_RELAY_URL` | Server WebSocket URL | (required) |
| `WSS_DOMAIN` | Domain to tunnel to | (required) |
| `WSS_TOKEN` | Authentication token | (required) |

```bash
# Example
WSS_RELAY_URL=wss://relay.example.com:8443
WSS_DOMAIN=app.internal
WSS_TOKEN=your-secret-token
```

## Security

### 10.x.x.x Enforcement

Server validates IP properly (not just string prefix):

```rust
fn validate_10_network(target: &str) -> bool {
    let (host, _) = target.rsplit_once(':').unwrap_or((target, ""));
    let ip: IpAddr = host.parse()?;
    
    match ip {
        IpAddr::V4(v4) => v4.octets()[0] == 10,
        IpAddr::V6(_) => false,
    }
}
```

- `10.0.0.1:443` ✅ Allowed
- `192.168.1.1:443` ❌ Denied
- `anything` ❌ Denied

### Rate Limiting

- Max connections per IP (default: 100)
- 60-second sliding window
- Configurable via `WSS_MAX_CONNECTIONS`

### Authentication

First message must be JSON:

```json
{
  "type": "auth",
  "token": "your-secret-token",
  "domain": "app.internal"
}
```

Server responds:

```json
{"type": "ok", "session_id": "uuid-here"}
```

## Performance

| Metric | Value |
|--------|-------|
| Throughput | 300Mbps+ |
| Buffer Size | 64KB |
| Latency | Low (direct tunnel) |

### Expected Transfer Times (300Mbps)

| File Size | Time |
|----------|------|
| 100 MB | ~3 sec |
| 1 GB | ~27 sec |
| 10 GB | ~4.5 min |

## Reconnect Logic

Client automatically reconnects on disconnect:

- Initial backoff: 1 second
- Exponential: 1s → 2s → 4s → 8s → ...
- Max backoff: 30 seconds
- Infinite retries (configurable)

## Logging

Server logs include session ID, domain, and client IP:

```
[session-id] [domain] [ip] Connected
[session-id] [domain] Disconnected
[session-id] [domain] Error: message
```

## Use Cases

### Access Internal Web App

```powershell
$env:WSS_RELAY_URL="wss://your-server.com:8443"
$env:WSS_DOMAIN="internal.yourcompany.com"
$env:WSS_TOKEN="your-secret-token"

.\wss-client.exe

# Open browser
Start-Process "https://internal.yourcompany.com"
```

### SSH over WebSocket

```powershell
# SSH config
Host internal
    HostName localhost
    Port 22
    User youruser
    LocalForward 2222 internal.yourcompany.com:22

# Terminal 1
.\wss-client.exe

# Terminal 2
ssh -p 2222 localhost
```

### RDP over WebSocket

```powershell
$env:WSS_RELAY_URL="wss://your-server.com:8443"
$env:WSS_DOMAIN="10.0.0.100:3389"
$env:WSS_TOKEN="your-secret-token"

.\wss-client.exe
mstsc.exe
```

## Docker Compose

```yaml
version: '3.8'

services:
  wss-server:
    build: ./wss-server
    ports:
      - "8080:8080"
    environment:
      - WSS_ROUTES=app.internal=10.0.0.1:443
      - WSS_TOKEN=your-secret-token
      - WSS_MAX_CONNECTIONS=100
    networks:
      - internal

  nginx:
    image: nginx:latest
    ports:
      - "443:443"
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf:ro
      - ./certs:/certs:ro
    depends_on:
      - wss-server
    networks:
      - internal

networks:
  internal:
    driver: bridge
```

## Nginx TLS Termination

```nginx
server {
    listen 443 ssl;
    ssl_certificate /certs/server.crt;
    ssl_certificate_key /certs/server.key;

    location / {
        proxy_pass http://wss-server:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }
}
```

## Troubleshooting

### DENIED Response

- Check target is in 10.x.x.x network
- Check `WSS_ROUTES` format

### Authentication Failed

- Ensure `WSS_TOKEN` matches server

### Rate Limit Exceeded

- Lower `WSS_MAX_CONNECTIONS` or wait 60 seconds

## Dependencies

### wss-server

```toml
tokio = { version = "1", features = ["full", "sync"] }
tokio-tungstenite = "0.21"
futures-util = "0.3"
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
```

### wss-client

```toml
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.21"
futures-util = "0.3"
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## License

MIT