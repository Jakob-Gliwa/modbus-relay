use std::{
    fmt::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

pub const BUCKETS_US: [u64; 11] = [
    100, 500, 1_000, 5_000, 10_000, 50_000, 100_000, 500_000, 1_000_000, 5_000_000, 10_000_000,
];

#[derive(Default)]
pub struct DeviceMetrics {
    pub clients: AtomicU64,
    pub connections: AtomicU64,
    pub rejected_clients: AtomicU64,
    pub queued: AtomicU64,
    pub requests: AtomicU64,
    pub invalid_requests: AtomicU64,
    pub request_deadlines: AtomicU64,
    pub responses: AtomicU64,
    pub exceptions: AtomicU64,
    pub failures: AtomicU64,
    pub timeouts: AtomicU64,
    pub protocol_errors: AtomicU64,
    pub client_errors: AtomicU64,
    pub retries: AtomicU64,
    pub busy: AtomicU64,
    pub expired: AtomicU64,
    pub repaired_exceptions: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub connected: AtomicBool,
    pub last_success_ms: AtomicU64,
    pub last_success_unix: AtomicU64,
    pub duration_us: AtomicU64,
    pub duration_count: AtomicU64,
    pub duration_buckets: [AtomicU64; 11],
}

pub struct Metrics {
    pub started: Instant,
    pub ready: AtomicBool,
    pub devices: Vec<(String, Arc<DeviceMetrics>)>,
    pub health_max_age_ms: u64,
}

impl Metrics {
    /// Passive response freshness. Failed attempts never refresh or invalidate
    /// the last valid response; zero means no response has been observed yet.
    pub fn healthy(&self, device: &DeviceMetrics) -> bool {
        let last = device.last_success_ms.load(Relaxed);
        last > 0 && self.now_ms().saturating_sub(last) <= self.health_max_age_ms
    }
    pub fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64 + 1
    }
    pub fn success(&self, device: &DeviceMetrics) {
        device.last_success_ms.store(self.now_ms(), Relaxed);
        device.last_success_unix.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Relaxed,
        );
    }
    pub fn render(&self) -> String {
        let mut text = String::with_capacity(4096 * self.devices.len());
        text.push_str("# HELP modbus_relay_build_info Build information.\n# TYPE modbus_relay_build_info gauge\n");
        let _ = writeln!(
            text,
            "modbus_relay_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        );
        text.push_str("# HELP modbus_relay_uptime_seconds Monotonic process uptime.\n# TYPE modbus_relay_uptime_seconds gauge\n");
        let _ = writeln!(
            text,
            "modbus_relay_uptime_seconds {}",
            self.started.elapsed().as_secs_f64()
        );
        macro_rules! metric {
            ($name:literal, $kind:literal, $help:literal, $value:expr) => {{
                let _ = writeln!(
                    text,
                    concat!(
                        "# HELP modbus_relay_",
                        $name,
                        " ",
                        $help,
                        "\n# TYPE modbus_relay_",
                        $name,
                        " ",
                        $kind
                    )
                );
                for (name, device) in &self.devices {
                    let value: u64 = ($value)(device.as_ref());
                    let _ = writeln!(
                        text,
                        concat!("modbus_relay_", $name, "{{device=\"{}\"}} {}"),
                        name, value
                    );
                }
            }};
        }
        metric!(
            "clients",
            "gauge",
            "Active client connections.",
            |d: &DeviceMetrics| d.clients.load(Relaxed)
        );
        metric!(
            "queue_depth",
            "gauge",
            "Requests waiting for the device worker.",
            |d: &DeviceMetrics| d.queued.load(Relaxed)
        );
        metric!(
            "backend_connected",
            "gauge",
            "Backend socket currently open; not an active probe.",
            |d: &DeviceMetrics| u64::from(d.connected.load(Relaxed))
        );
        metric!(
            "backend_healthy",
            "gauge",
            "A valid backend response was received within health_max_age; passive, including Modbus exceptions.",
            |d: &DeviceMetrics| u64::from(self.healthy(d))
        );
        metric!(
            "last_success_timestamp_seconds",
            "gauge",
            "Last valid backend response, Unix time, zero if unknown.",
            |d: &DeviceMetrics| d.last_success_unix.load(Relaxed)
        );
        macro_rules! counter {
            ($name:literal, $field:ident, $help:literal) => {
                metric!($name, "counter", $help, |d: &DeviceMetrics| d
                    .$field
                    .load(Relaxed));
            };
        }
        counter!(
            "backend_connections_total",
            connections,
            "Successful backend connections."
        );
        counter!(
            "rejected_clients_total",
            rejected_clients,
            "Clients refused at the configured limit."
        );
        counter!(
            "requests_total",
            requests,
            "Client requests with valid MBAP framing received."
        );
        counter!(
            "invalid_requests_total",
            invalid_requests,
            "Standard request PDUs rejected before forwarding."
        );
        counter!(
            "request_deadlines_total",
            request_deadlines,
            "Requests exceeding their total budget, including queueing."
        );
        counter!(
            "responses_total",
            responses,
            "Valid backend responses including exceptions."
        );
        counter!(
            "exceptions_total",
            exceptions,
            "Modbus exceptions returned by the device."
        );
        counter!("failures_total", failures, "Backend attempts that failed.");
        counter!(
            "timeouts_total",
            timeouts,
            "Backend attempts that timed out."
        );
        counter!(
            "protocol_errors_total",
            protocol_errors,
            "Invalid or mismatched backend frames."
        );
        counter!(
            "client_errors_total",
            client_errors,
            "Malformed, truncated or timed out client frames."
        );
        counter!("retries_total", retries, "Additional backend attempts.");
        counter!(
            "busy_total",
            busy,
            "Requests rejected because the queue is full."
        );
        counter!(
            "queue_expired_total",
            expired,
            "Requests expired before dispatch; never sent to device."
        );
        counter!(
            "repaired_exceptions_total",
            repaired_exceptions,
            "Exception MBAP lengths corrected and sockets reset."
        );
        counter!(
            "backend_bytes_sent_total",
            bytes_sent,
            "Bytes in fully written backend requests."
        );
        counter!(
            "backend_bytes_received_total",
            bytes_received,
            "Bytes in complete backend responses."
        );
        text.push_str("# HELP modbus_relay_request_duration_seconds Worker service time including retries, excluding queue wait.\n# TYPE modbus_relay_request_duration_seconds histogram\n");
        for (name, device) in &self.devices {
            for (bound, value) in BUCKETS_US.iter().zip(&device.duration_buckets) {
                let _ = writeln!(
                    text,
                    "modbus_relay_request_duration_seconds_bucket{{device=\"{name}\",le=\"{}\"}} {}",
                    *bound as f64 / 1e6,
                    value.load(Relaxed)
                );
            }
            let _ = writeln!(
                text,
                "modbus_relay_request_duration_seconds_bucket{{device=\"{name}\",le=\"+Inf\"}} {}\nmodbus_relay_request_duration_seconds_count{{device=\"{name}\"}} {}\nmodbus_relay_request_duration_seconds_sum{{device=\"{name}\"}} {}",
                device.duration_count.load(Relaxed),
                device.duration_count.load(Relaxed),
                device.duration_us.load(Relaxed) as f64 / 1e6
            );
        }
        text
    }
}

impl DeviceMetrics {
    pub fn observe(&self, elapsed: std::time::Duration) {
        let us = elapsed.as_micros() as u64;
        self.duration_us.fetch_add(us, Relaxed);
        for (bound, count) in BUCKETS_US.iter().zip(&self.duration_buckets) {
            if us <= *bound {
                count.fetch_add(1, Relaxed);
            }
        }
        self.duration_count.fetch_add(1, Relaxed);
    }
}
