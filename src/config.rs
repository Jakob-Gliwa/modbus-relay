use crate::Error;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    time::Duration,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub devices: Vec<Device>,
    #[serde(default)]
    pub monitoring: Monitoring,
    /// Accept Python dictConfig for migration, but only translate root.level.
    #[serde(default)]
    pub logging: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    #[serde(default)]
    pub name: String,
    pub modbus: Backend,
    pub listen: Option<Listen>,
    #[serde(default)]
    pub listeners: Vec<Listen>,
    #[serde(default)]
    pub unit_id_remapping: BTreeMap<u8, u8>,
    #[serde(default = "default_clients")]
    pub max_clients: usize,
    #[serde(default = "default_queue")]
    pub queue_capacity: usize,
    #[serde(default = "ten")]
    pub queue_timeout: f64,
    /// None derives a total request budget from existing timeouts and attempts.
    pub request_timeout: Option<f64>,
    #[serde(default = "idle_timeout")]
    pub client_idle_timeout: f64,
    #[serde(default = "five")]
    pub frame_timeout: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listen {
    pub bind: String,
}

impl Device {
    pub fn request_budget(&self) -> Duration {
        duration(self.request_timeout.unwrap_or_else(|| {
            self.queue_timeout
                + f64::from(self.modbus.attempts())
                    * (2.0 * self.modbus.timeout
                        + self.modbus.connection_time
                        + self.modbus.reconnect_delay
                        + self.modbus.request_delay)
        }))
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Tcp,
    Rtuovertcp,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    pub url: String,
    #[serde(default = "ten")]
    pub timeout: f64,
    #[serde(default)]
    pub connection_time: f64,
    #[serde(default)]
    pub connection_ttl: f64,
    #[serde(default)]
    pub idle_time: f64,
    #[serde(default)]
    pub reconnect_delay: f64,
    #[serde(default)]
    pub request_delay: f64,
    pub attempts: Option<u32>,
    pub retry_count: Option<u32>,
    #[serde(default)]
    pub retry_writes: bool,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub strip_rtu_echo: bool,
}

impl Backend {
    pub fn attempts(&self) -> u32 {
        self.attempts
            .or_else(|| self.retry_count.map(|n| n.saturating_add(1)))
            .unwrap_or(2)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Monitoring {
    pub bind: String,
    pub health_max_age: f64,
    pub max_connections: usize,
}
impl Default for Monitoring {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".into(),
            health_max_age: 60.0,
            max_connections: 16,
        }
    }
}

fn default_clients() -> usize {
    128
}
fn default_queue() -> usize {
    128
}
fn ten() -> f64 {
    10.0
}
fn five() -> f64 {
    5.0
}
fn idle_timeout() -> f64 {
    300.0
}
pub(crate) fn duration(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds)
}

pub fn address(value: &str, default_port: u16) -> Result<String, Error> {
    let value = value.strip_prefix("tcp://").unwrap_or(value);
    if value.contains("://")
        || value.contains(['/', '?', '#', '@'])
        || value.chars().any(char::is_whitespace)
    {
        return Err(format!("invalid TCP address: {value}").into());
    }
    let (host, port) = if value.starts_with('[') {
        let end = value
            .find(']')
            .ok_or("IPv6 addresses must use [host]:port")?;
        let host = &value[..=end];
        value[1..end].parse::<std::net::Ipv6Addr>()?;
        let tail = &value[end + 1..];
        (
            host,
            if tail.is_empty() {
                default_port
            } else {
                tail.strip_prefix(':').ok_or("invalid IPv6 port")?.parse()?
            },
        )
    } else {
        match value.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, port.parse::<u16>()?),
            None => (value, default_port),
            _ => return Err("IPv6 addresses must use [host]:port".into()),
        }
    };
    let host = if host.is_empty() || host == "0" {
        "0.0.0.0"
    } else {
        host
    };
    Ok(format!("{host}:{port}"))
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if std::fs::metadata(path)?.len() > 1024 * 1024 {
            return Err("config exceeds 1 MiB".into());
        }
        Self::parse(
            &std::fs::read_to_string(path)?,
            path.extension().and_then(|s| s.to_str()).unwrap_or(""),
        )
    }

    pub fn parse(text: &str, format: &str) -> Result<Self, Error> {
        let config: Self = match format {
            "yaml" | "yml" => serde_saphyr::from_str(text)?,
            "toml" => toml::from_str(text)?,
            "json" => serde_json::from_str(text)?,
            _ => return Err("config must use .yaml, .yml, .toml or .json".into()),
        };
        Ok(config)
    }

    pub fn validate(&mut self) -> Result<(), Error> {
        if self.devices.is_empty() || self.devices.len() > 128 {
            return Err("configure 1..=128 devices".into());
        }
        self.monitoring.bind = address(&self.monitoring.bind, 8080)?;
        check_time("health_max_age", self.monitoring.health_max_age, false)?;
        check_limit(
            "monitoring.max_connections",
            self.monitoring.max_connections,
            1024,
        )?;
        let mut names = HashSet::new();
        let mut endpoints = HashSet::new();
        let mut upstreams = HashSet::new();
        for (i, device) in self.devices.iter_mut().enumerate() {
            if device.name.is_empty() {
                device.name = format!("device_{}", i + 1);
            }
            if device.name.len() > 64
                || !device
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                || !names.insert(device.name.clone())
            {
                return Err("device names must be unique, 1..64 ASCII letters/digits/_/-".into());
            }
            let backend = &mut device.modbus;
            backend.url = address(&backend.url, 502)?;
            if !upstreams.insert(backend.url.clone()) {
                return Err(
                    "duplicate backend: use listeners on one device to share a connection".into(),
                );
            }
            if backend.attempts.is_some() && backend.retry_count.is_some() {
                return Err("use attempts or retry_count, not both".into());
            }
            if backend.retry_count.is_some_and(|n| n >= 100) {
                return Err("retry_count must be <100".into());
            }
            if !(1..=100).contains(&backend.attempts()) {
                return Err("attempts must be 1..=100".into());
            }
            check_time("timeout", backend.timeout, false)?;
            for (key, val) in [
                ("connection_time", backend.connection_time),
                ("connection_ttl", backend.connection_ttl),
                ("idle_time", backend.idle_time),
                ("reconnect_delay", backend.reconnect_delay),
                ("request_delay", backend.request_delay),
            ] {
                check_time(key, val, true)?;
            }
            if backend.strip_rtu_echo && backend.mode != Mode::Rtuovertcp {
                return Err("strip_rtu_echo requires mode: rtuovertcp".into());
            }
            check_limit("max_clients", device.max_clients, 65536)?;
            check_limit("queue_capacity", device.queue_capacity, 65536)?;
            check_time("queue_timeout", device.queue_timeout, false)?;
            if let Some(limit) = device.request_timeout {
                check_time("request_timeout", limit, false)?;
            }
            check_time("client_idle_timeout", device.client_idle_timeout, false)?;
            check_time("frame_timeout", device.frame_timeout, false)?;
            if device.listen.is_some() && !device.listeners.is_empty() {
                return Err("use listen or listeners, not both".into());
            }
            if let Some(listen) = device.listen.take() {
                device.listeners.push(listen);
            }
            if device.listeners.is_empty() || device.listeners.len() > 32 {
                return Err("configure 1..=32 listeners per device".into());
            }
            for listener in &mut device.listeners {
                listener.bind = address(&listener.bind, 502)?;
                if !listener.bind.ends_with(":0") && !endpoints.insert(listener.bind.clone()) {
                    return Err("duplicate listener address".into());
                }
            }
        }
        Ok(())
    }
}

fn check_time(name: &str, value: f64, zero: bool) -> Result<(), Error> {
    if !value.is_finite() || value > 86400.0 || if zero { value < 0.0 } else { value < 0.000001 } {
        return Err(format!(
            "{name} must be finite, {}..=86400 seconds",
            if zero { "0" } else { "0.000001" }
        )
        .into());
    }
    Ok(())
}
fn check_limit(name: &str, value: usize, max: usize) -> Result<(), Error> {
    if value == 0 || value > max {
        return Err(format!("{name} must be 1..={max}").into());
    }
    Ok(())
}
