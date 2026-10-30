use modbus_relay::metrics::{DeviceMetrics, Metrics};
use std::sync::atomic::Ordering::Relaxed;
use tokio::time::{Duration, Instant, advance};

#[tokio::test(start_paused = true)]
async fn health_freshness_boundary_unknown_and_recovery() {
    let metrics = Metrics {
        started: Instant::now(),
        ready: true.into(),
        devices: vec![],
        health_max_age_ms: 30_000,
    };
    let device = DeviceMetrics::default();
    assert!(!metrics.healthy(&device));
    metrics.success(&device);
    assert!(metrics.healthy(&device));
    advance(Duration::from_secs(30)).await;
    assert!(metrics.healthy(&device));
    advance(Duration::from_millis(1)).await;
    assert!(!metrics.healthy(&device));
    metrics.success(&device);
    assert!(metrics.healthy(&device));
    assert!(device.last_success_unix.load(Relaxed) > 0);
}
