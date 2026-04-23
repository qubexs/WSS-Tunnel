# WSS - WebSocket Tunnel

Secure WebSocket tunnel with domain-based routing and 10.x.x.x enforcement.

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
│  - TLS termination: nginx/traefik (port 443)       │
│  - Plain WS: wss-server (port 8080)                  │
│  - Domain routing: domain → 10.x.x.x:443            │
│  - Enforce: target must be 10.x.x.x               │
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

### 1. wss-server (Docker)

```bash
# Build
cd wss-server
cargo build --release
docker build -t wss-server .

# Run
docker run -p 8080:8080 \
  -e WSS_ROUTES="app.internal=10.0.0.1:443" \
  wss-server
```

With nginx TLS termination:

```nginx
# nginx.conf
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

### 2. wss-client

```bash
# Build
cd wss-client
cargo build --release

# Run
$env:WSS_RELAY_URL="wss://your-server:8443"
$env:WSS_DOMAIN="app.internal"
.\target\release\wss-client.exe
```

Or use .env:

```powershell
# .env
WSS_RELAY_URL=wss://your-server:8443
WSS_DOMAIN=app.internal
```

```powershell
Get-Content .env | ForEach-Object { $name, $value = $_ -split '='; Set-Item -Path "env:$name" -Value $Value }
.\wss-client.exe
```

## Environment Variables

### wss-server

| Variable | Description | Default |
|---------|------------|---------|
| `WSS_LISTEN` | Listen address | `0.0.0.0:8080` |
| `WSS_ROUTES` | Domain→target mappings | (required) |

Format: `domain1=target1,domain2=target2`

```bash
# Multiple routes
WSS_ROUTES=app1.internal=10.0.0.1:443,app2.internal=10.0.0.2:3389
```

### wss-client

| Variable | Description | Default |
|---------|------------|---------|
| `WSS_RELAY_URL` | Server WebSocket URL | (required) |
| `WSS_DOMAIN` | Domain to tunnel to | (required) |

```bash
# Example
WSS_RELAY_URL=wss://relay.example.com:8443
WSS_DOMAIN=app.internal
```

## Security

### 10.x.x.x Enforcement

Server only allows connections totargets starting with `10.`:

- `10.0.0.1:443` ✅ Allowed (HTTPS)
- `192.168.1.1:443` ❌ Denied
- `any-other:443` ❌ Denied

### TLS Flow

```
Client → wss:// (TLS) → Server → 10.x.x.x:443 (HTTPS)
                       → Target uses HTTPS (port 443)
```

All traffic is encrypted:
1. Client → Server: TLS (wss://)
2. Server → Target: HTTPS (port 443) with TLS

## Use Cases

### 1. Access Internal Web App

```powershell
# On local PC
$env:WSS_RELAY_URL="wss://your-server.com:8443"
$env:WSS_DOMAIN="internal.yourcompany.com"
.\wss-client.exe

# Now open browser
Start-Process "https://internal.yourcompany.com"
```

### 2. SSH over WebSocket

```powershell
# Configure SSH to use local port
# SSH config
Host internal
    HostName localhost
    Port 22
    User youruser
    LocalForward 2222 internal.yourcompany.com:22

# In terminal 1 - start tunnel
.\wss-client.exe

# In terminal 2 - SSH
ssh -p 2222 localhost
```

### 3. RDP over WebSocket

```powershell
# Connect to internal Windows via RDP
$env:WSS_RELAY_URL="wss://your-server.com:8443"
$env:WSS_DOMAIN="10.0.0.100:3389"

# Use RDP client to connect to localhost:3389
.\wss-client.exe
mstsc.exe
```

## Docker Compose Example

```yaml
version: '3.8'

services:
  wss-server:
    build: ./wss-server
    ports:
      - "8080:8080"
    environment:
      - WSS_ROUTES=app.internal=10.0.0.1:443
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

## Troubleshooting

### Connection Refused

```bash
# Check server is running
docker ps

# Check logs
docker logs <container>

# Check port
curl http://localhost:8080
```

### DENIED Response

- Target must start with `10.`
- Check `WSS_ROUTES` format

### TLS Errors

- For self-signed certs, install cert in Windows trust store
- Or use browser to accept cert manually

## Dependencies

### wss-server

```toml
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.21"
futures-util = "0.3"
anyhow = "1"
```

### wss-client

```toml
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.21"
futures-util = "0.3"
anyhow = "1"
```

## License

MIT