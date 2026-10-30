use crate::{
    Error,
    config::{Backend, Config, Device, Mode, duration},
    frame::{Frame, FrameReader, read_rtu, rtu_request, rtu_supported},
    metrics::{DeviceMetrics, Metrics},
    monitor,
};
use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep_until, timeout, timeout_at},
};

/// Owns every socket and task. Binding is transactional: nothing starts until all
/// listeners (including monitoring) have successfully bound.
pub struct Relay {
    pub listeners: Vec<Vec<SocketAddr>>,
    pub monitoring: SocketAddr,
    pub metrics: Arc<Metrics>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<io::Result<()>>>,
}

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

impl Relay {
    pub async fn start(mut config: Config) -> Result<Self, Error> {
        config.validate()?;
        let mut bound = Vec::new();
        let mut addresses = Vec::new();
        for device in &config.devices {
            let mut listeners = Vec::new();
            let mut local = Vec::new();
            for listen in &device.listeners {
                let listener = TcpListener::bind(&listen.bind).await?;
                local.push(listener.local_addr()?);
                listeners.push(listener);
            }
            addresses.push(local);
            bound.push(listeners);
        }
        let http = TcpListener::bind(&config.monitoring.bind).await?;
        let monitoring = http.local_addr()?;
        let metrics = Arc::new(Metrics {
            started: Instant::now(),
            ready: false.into(),
            health_max_age_ms: duration(config.monitoring.health_max_age).as_millis() as u64,
            devices: config
                .devices
                .iter()
                .map(|d| (d.name.clone(), Arc::new(DeviceMetrics::default())))
                .collect(),
        });
        let (stop, stopped) = watch::channel(false);
        let mut tasks = JoinSet::new();
        for ((device, listeners), (_, stats)) in
            config.devices.into_iter().zip(bound).zip(&metrics.devices)
        {
            let (tx, rx) = mpsc::channel(device.queue_capacity);
            let permits = Arc::new(Semaphore::new(device.max_clients));
            let device = Arc::new(device);
            tasks.spawn(worker(
                device.clone(),
                rx,
                stats.clone(),
                metrics.clone(),
                stopped.clone(),
            ));
            for listener in listeners {
                tasks.spawn(accept_clients(
                    listener,
                    device.clone(),
                    tx.clone(),
                    permits.clone(),
                    stats.clone(),
                    stopped.clone(),
                ));
            }
        }
        tasks.spawn(monitor::serve(
            http,
            metrics.clone(),
            config.monitoring.max_connections,
            stopped.clone(),
        ));
        metrics.ready.store(true, Relaxed);
        let shutdown = stop.clone();
        let state = metrics.clone();
        let task = tokio::spawn(async move {
            let mut stopped = stopped;
            let result = tokio::select! {
                biased;
                _ = stopped.changed() => Ok(()),
                result = tasks.join_next() => match result {
                    Some(Ok(Err(error))) => Err(error),
                    Some(Err(error)) => Err(io::Error::other(error)),
                    // Another runtime thread may finish a service after the
                    // stop branch was polled but before join_next was polled.
                    Some(Ok(Ok(()))) if *stopped.borrow() => Ok(()),
                    _ => Err(io::Error::other("service task stopped unexpectedly")),
                }
            };
            state.ready.store(false, Relaxed);
            let _ = shutdown.send(true);
            // Allow active exchanges to finish. Never replay an interrupted write.
            if timeout(Duration::from_secs(5), async {
                while tasks.join_next().await.is_some() {}
            })
            .await
            .is_err()
            {
                tasks.shutdown().await;
            }
            result
        });
        Ok(Self {
            listeners: addresses,
            monitoring,
            metrics,
            stop,
            task: Some(task),
        })
    }
    pub async fn shutdown(mut self) -> Result<(), Error> {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
    pub async fn wait(&mut self) -> Result<(), Error> {
        if let Some(task) = self.task.as_mut() {
            let result = task.await;
            self.task = None;
            result??;
        }
        Ok(())
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

struct ClientGuard(Arc<DeviceMetrics>);
impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Relaxed);
    }
}

struct Job {
    client_id: u64,
    request: Frame,
    deadline: Instant,
    request_deadline: Instant,
    reply: oneshot::Sender<Result<Frame, DeadlineExceeded>>,
    queued: QueueGuard,
}

struct DeadlineExceeded;
struct QueueGuard(Option<Arc<DeviceMetrics>>);
impl QueueGuard {
    fn finish(&mut self) {
        if let Some(stats) = self.0.take() {
            stats.queued.fetch_sub(1, Relaxed);
        }
    }
}
impl Drop for QueueGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

async fn accept_clients(
    listener: TcpListener,
    device: Arc<Device>,
    tx: mpsc::Sender<Job>,
    permits: Arc<Semaphore>,
    stats: Arc<DeviceMetrics>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            result = clients.join_next(), if !clients.is_empty() => { if let Some(Err(error)) = result { return Err(io::Error::other(error)); } },
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let Ok(permit) = permits.clone().try_acquire_owned() else { stats.rejected_clients.fetch_add(1, Relaxed); continue; };
                stream.set_nodelay(true)?;
                let sequence = NEXT_CLIENT_ID.fetch_add(1, Relaxed);
                tracing::debug!(device = %device.name, client = sequence, %peer, "client connected");
                stats.clients.fetch_add(1, Relaxed);
                let guard = ClientGuard(stats.clone());
                let (device, tx, stop) = (device.clone(), tx.clone(), stop.clone());
                clients.spawn(async move {
                    let (_permit, guard) = (permit, guard);
                    client(stream, device, tx, &guard.0, stop, sequence).await;
                });
            }
        }
    }
    // Clients blocked on idle reads see shutdown immediately. In-flight requests
    // can still receive their response; the supervisor enforces the grace limit.
    drop(listener);
    while let Some(result) = clients.join_next().await {
        result.map_err(io::Error::other)?;
    }
    Ok(())
}

async fn client(
    mut stream: TcpStream,
    device: Arc<Device>,
    tx: mpsc::Sender<Job>,
    stats: &Arc<DeviceMetrics>,
    mut stop: watch::Receiver<bool>,
    client_id: u64,
) {
    let mut reader = FrameReader::default();
    loop {
        if *stop.borrow() {
            break;
        }
        let read = reader.read_request(
            &mut stream,
            duration(device.client_idle_timeout),
            duration(device.frame_timeout),
        );
        let request = tokio::select! {
            biased;
            _ = stop.changed() => break,
            result = read => match result {
                Ok(Some(frame)) if frame.function() > 0 && frame.function() < 128 => frame,
                Ok(None) => break,
                _ => { stats.client_errors.fetch_add(1, Relaxed); break; }
            }
        };
        stats.requests.fetch_add(1, Relaxed);
        if let Err(code) = request.validate_request() {
            stats.invalid_requests.fetch_add(1, Relaxed);
            if !matches!(
                timeout(
                    duration(device.frame_timeout),
                    stream.write_all(request.exception(code).as_bytes())
                )
                .await,
                Ok(Ok(()))
            ) {
                break;
            }
            continue;
        }
        let request_deadline = Instant::now() + device.request_budget();
        let (reply, receiver) = oneshot::channel();
        // Reserve before constructing the job: no unbounded blocked senders and
        // exactly queue_capacity requests can be held by this channel.
        let response = match tx.try_reserve() {
            Ok(permit) => {
                let expired = request.exception(11);
                stats.queued.fetch_add(1, Relaxed);
                // Device metrics come from the channel worker's shared object.
                // The guard balances the gauge even on shutdown or task abort.
                let queued = QueueGuard(Some(stats.clone()));
                permit.send(Job {
                    client_id,
                    request,
                    deadline: Instant::now() + duration(device.queue_timeout),
                    request_deadline,
                    reply,
                    queued,
                });
                match timeout_at(request_deadline, receiver).await {
                    Ok(Ok(Ok(response))) => response,
                    Ok(Ok(Err(DeadlineExceeded))) | Err(_) => {
                        stats.request_deadlines.fetch_add(1, Relaxed);
                        expired
                    }
                    Ok(Err(_)) => break,
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                stats.busy.fetch_add(1, Relaxed);
                request.exception(6)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => break,
        };
        if !matches!(
            timeout(
                duration(device.frame_timeout),
                stream.write_all(response.as_bytes())
            )
            .await,
            Ok(Ok(()))
        ) {
            break;
        }
    }
}

struct Connection {
    stream: TcpStream,
    reader: FrameReader,
    born: Instant,
    last: Instant,
    stats: Arc<DeviceMetrics>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.stats.connected.store(false, Relaxed);
    }
}

struct Worker {
    config: Arc<Device>,
    connection: Option<Connection>,
    stats: Arc<DeviceMetrics>,
    metrics: Arc<Metrics>,
    next_tid: u16,
    next_connect: Instant,
    next_request: Instant,
    in_flight: bool,
}

async fn worker(
    config: Arc<Device>,
    mut rx: mpsc::Receiver<Job>,
    stats: Arc<DeviceMetrics>,
    metrics: Arc<Metrics>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut worker = Worker {
        config,
        connection: None,
        stats,
        metrics,
        next_tid: 0,
        next_connect: Instant::now(),
        next_request: Instant::now(),
        in_flight: false,
    };
    loop {
        if *stop.borrow() {
            break;
        }
        let expiry = worker.expiry();
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            _ = async { if let Some(deadline) = expiry { sleep_until(deadline).await; } else { std::future::pending::<()>().await; } } => { worker.connection = None; },
            job = rx.recv() => {
                let Some(mut job) = job else { break; };
                job.queued.finish();
                if job.reply.is_closed() { continue; }
                if Instant::now() >= job.request_deadline {
                    let _ = job.reply.send(Err(DeadlineExceeded));
                    continue;
                }
                if Instant::now() >= job.deadline {
                    worker.stats.expired.fetch_add(1, Relaxed);
                    let _ = job.reply.send(Ok(job.request.exception(6)));
                    continue;
                }
                let start = Instant::now();
                let response = timeout_at(job.request_deadline, worker.process(job.request, job.client_id, job.request_deadline))
                    .await.unwrap_or(Err(DeadlineExceeded));
                if response.is_err() {
                    // A cancelled read/write may have consumed only part of an ADU.
                    worker.connection = None;
                    if worker.in_flight {
                        worker.next_request = Instant::now() + duration(worker.config.modbus.request_delay);
                        worker.next_connect = Instant::now() + duration(worker.config.modbus.reconnect_delay);
                    }
                    worker.in_flight = false;
                }
                worker.stats.observe(start.elapsed());
                let _ = job.reply.send(response);
            }
        }
    }
    Ok(())
}

impl Worker {
    fn expiry(&self) -> Option<Instant> {
        let connection = self.connection.as_ref()?;
        let config = &self.config.modbus;
        let idle = (config.idle_time > 0.0).then(|| connection.last + duration(config.idle_time));
        let ttl = (config.connection_ttl > 0.0)
            .then(|| connection.born + duration(config.connection_ttl));
        match (idle, ttl) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    async fn process(
        &mut self,
        original: Frame,
        client_id: u64,
        deadline: Instant,
    ) -> Result<Frame, DeadlineExceeded> {
        let backend = &self.config.modbus;
        if backend.mode == Mode::Rtuovertcp
            && (!rtu_supported(original.function())
                || self
                    .config
                    .unit_id_remapping
                    .get(&original.unit())
                    .copied()
                    .unwrap_or(original.unit())
                    == 0)
        {
            return Ok(original.exception(1));
        }
        let attempts = if original.is_read_only() || backend.retry_writes {
            backend.attempts()
        } else {
            1
        };
        for attempt in 0..attempts {
            if Instant::now() >= deadline {
                return Err(DeadlineExceeded);
            }
            if attempt > 0 {
                self.stats.retries.fetch_add(1, Relaxed);
            }
            let ready_at = self.next_request.max(self.next_connect);
            // Tokio timers round to milliseconds. Registering an already-due
            // deadline would add a timer tick to every unpaced request.
            if ready_at > Instant::now() {
                sleep_until(ready_at).await;
            }
            if Instant::now() >= deadline {
                return Err(DeadlineExceeded);
            }
            if self.expiry().is_some_and(|expiry| Instant::now() >= expiry) {
                self.connection = None;
            }
            let mut request = original.clone();
            request.set_unit(
                self.config
                    .unit_id_remapping
                    .get(&original.unit())
                    .copied()
                    .unwrap_or(original.unit()),
            );
            self.next_tid = self.next_tid.wrapping_add(1);
            request.set_tid(self.next_tid);
            tracing::debug!(device = %self.config.name, client = client_id, tid = original.tid(), backend_tid = request.tid(), unit = request.unit(), function = request.function(), "dispatching request");
            let result = self.attempt(&request, deadline).await;
            if Instant::now() >= deadline {
                return Err(DeadlineExceeded);
            }
            self.in_flight = false;
            self.next_request = Instant::now() + duration(self.config.modbus.request_delay);
            match result {
                Ok((mut response, repaired)) => {
                    self.stats.responses.fetch_add(1, Relaxed);
                    if response.function() & 0x80 != 0 {
                        self.stats.exceptions.fetch_add(1, Relaxed);
                    }
                    if repaired {
                        self.stats.repaired_exceptions.fetch_add(1, Relaxed);
                        // Closing also discards padding that arrives AFTER the frame.
                        self.connection = None;
                    }
                    self.metrics.success(&self.stats);
                    response.set_tid(original.tid());
                    response.set_unit(original.unit());
                    return Ok(response);
                }
                Err(error) => {
                    self.connection = None;
                    self.stats.failures.fetch_add(1, Relaxed);
                    match error.kind() {
                        io::ErrorKind::TimedOut => {
                            self.stats.timeouts.fetch_add(1, Relaxed);
                        }
                        io::ErrorKind::InvalidData => {
                            self.stats.protocol_errors.fetch_add(1, Relaxed);
                        }
                        _ => {}
                    }
                    self.next_connect =
                        Instant::now() + duration(self.config.modbus.reconnect_delay);
                    tracing::warn!(device = %self.config.name, client = client_id, tid = original.tid(), backend_tid = request.tid(), attempt = attempt + 1, %error, "backend exchange failed; socket discarded");
                }
            }
        }
        Ok(original.exception(0x0b))
    }

    async fn attempt(&mut self, request: &Frame, deadline: Instant) -> io::Result<(Frame, bool)> {
        let backend = &self.config.modbus;
        if self.connection.is_none() {
            let stream =
                timeout(duration(backend.timeout), TcpStream::connect(&backend.url)).await??;
            stream.set_nodelay(true)?;
            self.stats.connections.fetch_add(1, Relaxed);
            self.stats.connected.store(true, Relaxed);
            self.connection = Some(Connection {
                stream,
                reader: FrameReader::default(),
                born: Instant::now(),
                last: Instant::now(),
                stats: self.stats.clone(),
            });
            if backend.connection_time > 0.0 {
                tokio::time::sleep(duration(backend.connection_time)).await;
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "request expired before send",
            ));
        }
        self.in_flight = true;
        let connection = self.connection.as_mut().expect("connection established");
        let response = timeout(
            duration(backend.timeout),
            exchange(
                &mut connection.stream,
                &mut connection.reader,
                request,
                backend,
                &self.stats,
            ),
        )
        .await??;
        connection.last = Instant::now();
        Ok(response)
    }
}

async fn exchange(
    stream: &mut TcpStream,
    reader: &mut FrameReader,
    request: &Frame,
    config: &Backend,
    stats: &DeviceMetrics,
) -> io::Result<(Frame, bool)> {
    let (response, repaired) = if config.mode == Mode::Tcp {
        stream.write_all(request.as_bytes()).await?;
        stats
            .bytes_sent
            .fetch_add(request.as_bytes().len() as u64, Relaxed);
        let response = reader
            .read(stream, true)
            .await?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "backend disconnected"))?;
        stats
            .bytes_received
            .fetch_add(response.0.as_bytes().len() as u64, Relaxed);
        response
    } else {
        let (bytes, len) = rtu_request(request);
        stream.write_all(&bytes[..len]).await?;
        stats.bytes_sent.fetch_add(len as u64, Relaxed);
        if config.strip_rtu_echo {
            let mut echo = [0; 256];
            stream.read_exact(&mut echo[..len]).await?;
            if echo[..len] != bytes[..len] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "RTU echo differs from request",
                ));
            }
        }
        let frame = read_rtu(stream, request.tid()).await?;
        stats
            .bytes_received
            .fetch_add((frame.as_bytes().len() - 4) as u64, Relaxed);
        (frame, false)
    };
    response.validate_response(request)?;
    Ok((response, repaired))
}
