//! Reproducible loopback benchmark. The proxy runs in a separate process so its
//! RSS excludes the mock PLC and load generator. No real device is contacted.
use clap::Parser;
use modbus_relay::{Error, frame::Frame};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::Barrier,
    task::JoinSet,
    time::{Instant, sleep, timeout},
};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "target/release/modbus-relay")]
    binary: PathBuf,
    /// Compare against the upstream Python source using the same workload.
    #[arg(long)]
    python_source: Option<PathBuf>,
    #[arg(long, default_value_t=16, value_parser=clap::value_parser!(u16).range(1..=128))]
    clients: u16,
    #[arg(long, default_value_t=2000, value_parser=clap::value_parser!(u32).range(1..=1000000))]
    requests_per_client: u32,
    #[arg(long, default_value_t = 1)]
    workers: u16,
    #[arg(long, default_value_t = 0)]
    device_delay_us: u64,
    /// Identical logging level for every compared proxy binary.
    #[arg(long, default_value = "info")]
    log_level: String,
    /// Capture logs and verify that debug dispatch messages were not dropped.
    #[arg(long)]
    log_file: Option<PathBuf>,
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Error> {
    let args = Args::parse();
    let device = TcpListener::bind("127.0.0.1:0").await?;
    let device_addr = device.local_addr()?;
    let upstream_connections = Arc::new(AtomicU64::new(0));
    let count = upstream_connections.clone();
    let backend = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                accepted=device.accept()=>{
                    let (mut stream,_)=accepted.unwrap(); stream.set_nodelay(true).unwrap(); count.fetch_add(1,Relaxed);
                    tasks.spawn(async move {
                        while let Ok((request,_))=Frame::read(&mut stream,false).await {
                            if args.device_delay_us>0 { sleep(Duration::from_micros(args.device_delay_us)).await; }
                            let reply=Frame::new(request.tid(),request.unit(),&[3,2,request.pdu()[1],request.pdu()[2]]).unwrap();
                            if stream.write_all(reply.as_bytes()).await.is_err() { break; }
                        }
                    });
                }
                result=tasks.join_next(), if !tasks.is_empty()=>{ result.unwrap().unwrap(); }
            }
        }
    });
    let reservation = TcpListener::bind("127.0.0.1:0").await?;
    let bind = reservation.local_addr()?;
    drop(reservation);
    let legacy = args.python_source.is_some();
    let mut command = if let Some(path) = args.python_source {
        let mut cmd = Command::new("python3");
        cmd.arg(path);
        cmd
    } else {
        Command::new(args.binary)
    };
    command.args([
        "--modbus",
        &device_addr.to_string(),
        "-b",
        &bind.to_string(),
    ]);
    if !legacy {
        command.args([
            "--monitoring-bind",
            "127.0.0.1:0",
            "--workers",
            &args.workers.to_string(),
        ]);
    }
    command.env("RUST_LOG", &args.log_level);
    let stderr = match &args.log_file {
        Some(path) => Stdio::from(std::fs::File::create(path)?),
        None => Stdio::null(),
    };
    let mut proxy = Process(command.stdout(Stdio::null()).stderr(stderr).spawn()?);
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = proxy.0.try_wait()? {
                return Err(std::io::Error::other(format!("proxy exited: {status}")));
            }
            if TcpStream::connect(bind).await.is_ok() {
                return Ok::<_, std::io::Error>(());
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    // Warm caches and establish the single backend connection before sampling.
    let mut warm = TcpStream::connect(bind).await?;
    warm.set_nodelay(true)?;
    for n in 0..100 {
        let request = Frame::new(n, 1, &[3, 0, 0, 0, 1])?;
        warm.write_all(request.as_bytes()).await?;
        Frame::read(&mut warm, true).await?;
    }
    drop(warm);
    let peak = Arc::new(AtomicU64::new(0));
    let sampled = peak.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let pid = proxy.0.id();
    let sampler = tokio::task::spawn_blocking(move || {
        while !done.load(Relaxed) {
            // Reading procfs avoids spawning a process for every Linux sample.
            #[cfg(target_os = "linux")]
            let rss = std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|s| {
                    s.lines().find_map(|line| {
                        line.strip_prefix("VmRSS:")?
                            .split_whitespace()
                            .next()?
                            .parse::<u64>()
                            .ok()
                    })
                });
            #[cfg(not(target_os = "linux"))]
            let rss = Command::new("ps")
                .args(["-o", "rss=", "-p", &pid.to_string()])
                .output()
                .ok()
                .and_then(|output| {
                    String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                        .ok()
                });
            if let Some(rss) = rss {
                sampled.fetch_max(rss, Relaxed);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    let barrier = Arc::new(Barrier::new(usize::from(args.clients) + 1));
    let mut clients = JoinSet::new();
    for id in 0..args.clients {
        let barrier = barrier.clone();
        let mut stream = TcpStream::connect(bind).await?;
        stream.set_nodelay(true)?;
        clients.spawn(async move {
            let mut latencies = Vec::with_capacity(args.requests_per_client as usize);
            barrier.wait().await;
            for n in 0..args.requests_per_client {
                let request =
                    Frame::new(n as u16, 1, &[3, (id >> 8) as u8, id as u8, 0, 1]).unwrap();
                let start = Instant::now();
                stream.write_all(request.as_bytes()).await.unwrap();
                let response = timeout(Duration::from_secs(30), Frame::read(&mut stream, true))
                    .await
                    .unwrap()
                    .unwrap()
                    .0;
                assert_eq!(response.tid(), n as u16);
                assert_eq!(response.pdu(), &[3, 2, (id >> 8) as u8, id as u8]);
                latencies.push(start.elapsed().as_micros() as u64);
            }
            latencies
        });
    }
    let start = Instant::now();
    barrier.wait().await;
    let mut latencies = Vec::new();
    while let Some(result) = clients.join_next().await {
        latencies.extend(result?);
    }
    let elapsed = start.elapsed().as_secs_f64();
    finished.store(true, Relaxed);
    sampler.await?;
    // Flush asynchronous loggers before checking message counts. Exclude shutdown
    // from the throughput interval; all measured replies have already arrived.
    #[cfg(unix)]
    {
        if !Command::new("sh")
            .args(["-c", "kill -TERM \"$1\"", "benchmark", &pid.to_string()])
            .status()?
            .success()
        {
            return Err("failed to stop proxy".into());
        }
        timeout(Duration::from_secs(8), async {
            loop {
                if let Some(status) = proxy.0.try_wait()? {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(std::io::Error::other(format!("proxy exited: {status}")))
                    };
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await??;
    }
    let logged_dispatches = args
        .log_file
        .as_ref()
        .map(|path| {
            std::fs::read_to_string(path).map(|text| {
                text.lines()
                    .filter(|line| line.contains("dispatching request"))
                    .count()
            })
        })
        .transpose()?;
    if args.log_level == "debug"
        && let Some(count) = logged_dispatches
    {
        assert_eq!(
            count,
            latencies.len() + 100,
            "logger dropped dispatch records"
        );
    }
    latencies.sort_unstable();
    let percentile = |p: usize| latencies[(latencies.len() - 1) * p / 100];
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "proxy":if legacy {"python-upstream"} else {"rust"},"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
            "clients":args.clients,"requests":latencies.len(),"workers":if legacy {1} else {args.workers},
            "device_delay_us":args.device_delay_us,"elapsed_seconds":elapsed,"requests_per_second":latencies.len() as f64/elapsed,
            "latency_us":{"p50":percentile(50),"p95":percentile(95),"p99":percentile(99)},
            "peak_sampled_rss_kib":peak.load(Relaxed),"backend_connections":upstream_connections.load(Relaxed),
            "log_level":args.log_level,"logged_dispatches":logged_dispatches,
            "note":"Loopback synthetic device; RSS sampled every 25ms via procfs (Linux) or ps; not a production guarantee. Tokio sleep has millisecond granularity."
        }))?
    );
    backend.abort();
    let _ = backend.await;
    Ok(())
}
