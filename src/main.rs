use clap::Parser;
use modbus_relay::{Error, Relay, config::Config};
use std::{io, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Bounded Modbus TCP proxy with health and Prometheus endpoints"
)]
struct Args {
    #[arg(short = 'c', long = "config-file", alias = "config")]
    config: Option<PathBuf>,
    #[arg(short, long, requires = "modbus")]
    bind: Option<String>,
    #[arg(long)]
    modbus: Option<String>,
    #[arg(long, default_value_t = 10.0)]
    timeout: f64,
    #[arg(long, default_value_t = 0.0)]
    modbus_connection_time: f64,
    #[arg(long, default_value_t = 0.0)]
    modbus_connection_ttl: f64,
    #[arg(long, default_value_t = 0.0)]
    modbus_idle_time: f64,
    #[arg(long, alias = "modbus-reconnect-delay", default_value_t = 0.0)]
    reconnect_delay: f64,
    #[arg(long, alias = "modbus-request-delay", default_value_t = 0.0)]
    request_delay: f64,
    #[arg(long, default_value_t = 2)]
    attempts: u32,
    #[arg(long)]
    retry_writes: bool,
    #[arg(long)]
    monitoring_bind: Option<String>,
    /// One event-loop thread by default; increase for many independent devices.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=128))]
    workers: u16,
    /// Validate configuration and exit without opening any sockets.
    #[arg(long)]
    check: bool,
    /// HTTP liveness probe for scratch containers (address, e.g. 127.0.0.1:8080).
    #[arg(long)]
    healthcheck: Option<String>,
}

fn main() -> Result<(), Error> {
    let args = Args::parse();
    let runtime = if args.workers == 1 {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
    } else {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(args.workers.into())
            .enable_all()
            .build()?
    };
    runtime.block_on(run(args))
}

async fn run(args: Args) -> Result<(), Error> {
    if let Some(address) = args.healthcheck {
        timeout(Duration::from_secs(2), async {
            let mut stream = TcpStream::connect(address).await?;
            stream
                .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await?;
            let mut status = [0; 12];
            stream.read_exact(&mut status).await?;
            if &status != b"HTTP/1.1 200" {
                return Err(io::Error::other("healthcheck failed"));
            }
            Ok::<_, io::Error>(())
        })
        .await??;
        return Ok(());
    }
    let mut config = if let Some(path) = args.config {
        Config::load(path)?
    } else {
        Config {
            devices: vec![],
            monitoring: Default::default(),
            logging: None,
        }
    };
    if let Some(url) = args.modbus {
        config.devices.push(serde_json::from_value(serde_json::json!({
            "modbus": {"url":url, "timeout":args.timeout, "connection_time":args.modbus_connection_time,
                "connection_ttl":args.modbus_connection_ttl, "idle_time":args.modbus_idle_time,
                "reconnect_delay":args.reconnect_delay, "request_delay":args.request_delay,
                "attempts":args.attempts, "retry_writes":args.retry_writes},
            "listen": {"bind":args.bind.unwrap_or_else(|| "0.0.0.0:502".into())}
        }))?);
    }
    if let Some(bind) = args.monitoring_bind {
        config.monitoring.bind = bind;
    }
    config.validate()?;
    let log_level = config
        .logging
        .as_ref()
        .and_then(|v| v.get("root")?.get("level")?.as_str())
        .unwrap_or("info")
        .to_lowercase();
    let log_level = match log_level.as_str() {
        "warning" => "warn",
        "critical" => "error",
        _ => &log_level,
    };
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<tracing_subscriber::filter::Targets>().ok())
        .unwrap_or_else(|| {
            log_level.parse().unwrap_or_else(|_| {
                tracing_subscriber::filter::Targets::new().with_default(tracing::Level::INFO)
            })
        });
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr),
        )
        .try_init()?;
    if config.logging.is_some() {
        tracing::warn!(
            "Python logging config: only root.level is used; output is structured stderr, use RUST_LOG for filtering"
        );
    }
    for device in &config.devices {
        if device.modbus.retry_count.is_some() {
            tracing::warn!(device = %device.name, "retry_count is deprecated; use attempts = retry_count + 1");
        }
    }
    if args.check {
        println!("configuration valid ({} devices)", config.devices.len());
        return Ok(());
    }
    let mut relay = Relay::start(config).await?;
    tracing::info!(listeners = ?relay.listeners, monitoring = %relay.monitoring, "modbus-relay ready");
    tokio::select! {
        result = relay.wait() => result?,
        result = shutdown_signal() => { result?; tracing::info!("shutting down"); relay.shutdown().await?; }
    }
    Ok(())
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
