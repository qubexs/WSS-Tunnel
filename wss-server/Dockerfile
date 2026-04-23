FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY wss-server /app/wss-server

EXPOSE 8080

ENV WSS_LISTEN=0.0.0.0:8080
ENV WSS_ROUTES=app.internal=10.0.0.1:443

CMD ["/app/wss-server"]