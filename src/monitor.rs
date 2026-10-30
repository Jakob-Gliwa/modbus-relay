//! Deliberately small, bounded HTTP/1 management server. One request per socket.
use crate::metrics::Metrics;
use std::{
    io,
    sync::{Arc, atomic::Ordering::Relaxed},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    task::JoinSet,
    time::timeout,
};

pub(crate) async fn serve(
    listener: TcpListener,
    metrics: Arc<Metrics>,
    max: usize,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let permits = Arc::new(Semaphore::new(max));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            result = connections.join_next(), if !connections.is_empty() => { if let Some(Err(error)) = result { return Err(io::Error::other(error)); } },
            result = listener.accept() => {
                let (stream, _) = result?;
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let metrics = metrics.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let _ = timeout(Duration::from_secs(3), handle(stream, metrics)).await;
                });
            }
        }
    }
    connections.shutdown().await;
    Ok(())
}

async fn handle(mut stream: TcpStream, metrics: Arc<Metrics>) -> io::Result<()> {
    let mut buf = [0; 4096];
    let mut size = 0;
    let (method, path) = loop {
        if size == buf.len() {
            return respond(&mut stream, 431, "text/plain", "headers too large\n", false).await;
        }
        let read = stream.read(&mut buf[size..]).await?;
        if read == 0 {
            return Ok(());
        }
        size += read;
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&buf[..size]) {
            Ok(httparse::Status::Partial) => continue,
            Ok(httparse::Status::Complete(_)) => {
                if request.headers.iter().any(|h| {
                    h.name.eq_ignore_ascii_case("transfer-encoding")
                        || h.name.eq_ignore_ascii_case("content-length") && h.value != b"0"
                }) {
                    return respond(
                        &mut stream,
                        400,
                        "text/plain",
                        "request body unsupported\n",
                        false,
                    )
                    .await;
                }
                break (
                    request.method.unwrap_or("").to_owned(),
                    request.path.unwrap_or("").to_owned(),
                );
            }
            Err(_) => {
                return respond(&mut stream, 400, "text/plain", "invalid request\n", false).await;
            }
        }
    };
    let head = method == "HEAD";
    if method != "GET" && !head {
        return respond(&mut stream, 405, "text/plain", "use GET or HEAD\n", false).await;
    }
    let ready = metrics.ready.load(Relaxed);
    match path.as_str() {
        "/healthz" | "/livez" | "/readyz" => {
            respond(
                &mut stream,
                if ready { 200 } else { 503 },
                "application/json",
                if ready {
                    "{\"status\":\"ok\"}\n"
                } else {
                    "{\"status\":\"stopping\"}\n"
                },
                head,
            )
            .await
        }
        "/health" => {
            let devices: Vec<_> = metrics.devices.iter().map(|(name, d)| serde_json::json!({"name": name, "healthy": metrics.healthy(d), "connected": d.connected.load(Relaxed), "last_success_timestamp": d.last_success_unix.load(Relaxed)})).collect();
            let healthy = ready && metrics.devices.iter().all(|(_, d)| metrics.healthy(d));
            let body = serde_json::json!({"status": if healthy {"ok"} else {"degraded"}, "devices": devices}).to_string() + "\n";
            respond(
                &mut stream,
                if healthy { 200 } else { 503 },
                "application/json",
                &body,
                head,
            )
            .await
        }
        "/metrics" => {
            respond(
                &mut stream,
                200,
                "text/plain; version=0.0.4; charset=utf-8",
                &metrics.render(),
                head,
            )
            .await
        }
        _ => respond(&mut stream, 404, "text/plain", "not found\n", head).await,
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    head: bool,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Service Unavailable",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nAllow: GET, HEAD\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    if !head {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.shutdown().await
}
