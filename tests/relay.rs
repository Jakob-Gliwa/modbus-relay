//! Real TCP integration tests; ephemeral ports, explicit synchronization, bounded
//! waits, and task guards keep failures reproducible without external hardware.
use modbus_relay::{
    Relay,
    config::Config,
    frame::{Frame, rtu_request},
};
use serde_json::{Value, json};
use std::{
    future::Future,
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
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep, timeout},
};

struct Task(JoinHandle<()>);
impl Task {
    async fn finish(mut self) {
        timeout(Duration::from_secs(3), &mut self.0)
            .await
            .expect("mock backend did not finish")
            .expect("mock backend panicked");
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn spawn(future: impl Future<Output = ()> + Send + 'static) -> Task {
    Task(tokio::spawn(future))
}
async fn bound() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}
fn config(address: SocketAddr) -> Value {
    json!({"monitoring":{"bind":"127.0.0.1:0"},"devices":[{"name":"plc","modbus":{"url":address.to_string(),"timeout":0.25,"attempts":1},"listen":{"bind":"127.0.0.1:0"}}]})
}
async fn start(value: Value) -> Relay {
    Relay::start(serde_json::from_value::<Config>(value).unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_multithreaded_shutdown_is_successful() {
    let address = "127.0.0.1:1".parse().unwrap();
    for _ in 0..200 {
        let relay = start(config(address)).await;
        timeout(Duration::from_secs(3), relay.shutdown())
            .await
            .expect("shutdown stalled")
            .expect("normal shutdown reported a service failure");
    }
}

fn request(tid: u16, unit: u8, value: u16) -> Frame {
    Frame::new(tid, unit, &[3, (value >> 8) as u8, value as u8, 0, 1]).unwrap()
}
fn response(request: &Frame) -> Frame {
    Frame::new(
        request.tid(),
        request.unit(),
        &[request.function(), 2, request.pdu()[1], request.pdu()[2]],
    )
    .unwrap()
}
async fn send(stream: &mut TcpStream, frame: &Frame) {
    timeout(Duration::from_secs(3), stream.write_all(frame.as_bytes()))
        .await
        .unwrap()
        .unwrap();
}
async fn receive(stream: &mut TcpStream) -> Frame {
    timeout(Duration::from_secs(3), Frame::read(stream, true))
        .await
        .unwrap()
        .unwrap()
        .0
}
async fn connect(relay: &Relay) -> TcpStream {
    let stream = TcpStream::connect(relay.listeners[0][0]).await.unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}
async fn exchange(stream: &mut TcpStream, frame: &Frame) -> Frame {
    send(stream, frame).await;
    receive(stream).await
}
async fn wait_for(mut condition: impl FnMut() -> bool) {
    timeout(Duration::from_secs(3), async {
        while !condition() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
async fn http(relay: &Relay, path: &str) -> String {
    raw_http(relay, &format!("GET {path} HTTP/1.1\r\nHost: test\r\n\r\n")).await
}
async fn raw_http(relay: &Relay, request: &str) -> String {
    let mut stream = TcpStream::connect(relay.monitoring).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(4), stream.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}
async fn echo() -> (SocketAddr, Arc<AtomicU64>, Task) {
    let listener = bound().await;
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicU64::new(0));
    let count = connections.clone();
    let task = spawn(async move {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                result=listener.accept()=>{
                    let (mut stream,_)=result.unwrap(); count.fetch_add(1,Relaxed); stream.set_nodelay(true).unwrap();
                    tasks.spawn(async move { while let Ok((request,_))=Frame::read(&mut stream,false).await { if stream.write_all(response(&request).as_bytes()).await.is_err() { break; } } });
                }
                _=tasks.join_next(), if !tasks.is_empty()=>{}
            }
        }
    });
    (addr, connections, task)
}

#[tokio::test]
async fn forwards_fragmented_pipeline_and_vendor_pdu() {
    let listener = bound().await;
    let relay = start(config(listener.local_addr().unwrap())).await;
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        for _ in 0..3 {
            let frame = Frame::read(&mut stream, false).await.unwrap().0;
            let reply = if frame.function() == 3 {
                response(&frame)
            } else {
                frame
            };
            for byte in reply.as_bytes() {
                stream.write_all(&[*byte]).await.unwrap();
            }
        }
    });
    let mut client = connect(&relay).await;
    let a = request(9, 1, 2);
    let b = request(10, 2, 3);
    for byte in a.as_bytes() {
        client.write_all(&[*byte]).await.unwrap();
    }
    send(&mut client, &b).await;
    assert_eq!(
        receive(&mut client).await.as_bytes(),
        response(&a).as_bytes()
    );
    assert_eq!(
        receive(&mut client).await.as_bytes(),
        response(&b).as_bytes()
    );
    let vendor = Frame::new(0xffff, 240, &[65, 0, 99, 123]).unwrap();
    assert_eq!(
        exchange(&mut client, &vendor).await.as_bytes(),
        vendor.as_bytes()
    );
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_clients_reusing_transaction_ids_share_one_backend() {
    let (addr, count, _task) = echo().await;
    let relay = start(config(addr)).await;
    let mut clients = JoinSet::new();
    for unit in 1..=32 {
        let mut stream = connect(&relay).await;
        clients.spawn(async move {
            for n in 1..=20 {
                let req = request(1, unit, n);
                assert_eq!(
                    exchange(&mut stream, &req).await.as_bytes(),
                    response(&req).as_bytes()
                );
            }
        });
    }
    while let Some(result) = clients.join_next().await {
        result.unwrap();
    }
    assert_eq!(count.load(Relaxed), 1);
    assert_eq!(relay.metrics.devices[0].1.responses.load(Relaxed), 640);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn many_to_one_mapping_restores_each_request_unit() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["unit_id_remapping"] = json!({"1":0,"2":0});
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        for _ in 0..3 {
            let req = Frame::read(&mut stream, false).await.unwrap().0;
            assert_eq!(req.unit(), 0);
            stream.write_all(response(&req).as_bytes()).await.unwrap();
        }
    });
    let mut client = connect(&relay).await;
    for unit in [1, 2, 0] {
        assert_eq!(
            exchange(&mut client, &request(22, unit, 7)).await.unit(),
            unit
        );
    }
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn mismatched_tid_unit_function_and_protocol_reset_connection() {
    for mismatch in ["tid", "unit", "function", "protocol"] {
        let listener = bound().await;
        let relay = start(config(listener.local_addr().unwrap())).await;
        let backend = spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let req = Frame::read(&mut first, false).await.unwrap().0;
            let mut bytes = response(&req).as_bytes().to_vec();
            match mismatch {
                "tid" => bytes[1] ^= 1,
                "unit" => bytes[6] ^= 1,
                "function" => bytes[7] = 4,
                _ => bytes[3] = 1,
            }
            first.write_all(&bytes).await.unwrap();
            let mut b = [0];
            match first.read(&mut b).await {
                Ok(0) => {}
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("backend was not closed: {other:?}"),
            }
            let (mut second, _) = listener.accept().await.unwrap();
            let req = Frame::read(&mut second, false).await.unwrap().0;
            second.write_all(response(&req).as_bytes()).await.unwrap();
        });
        let mut client = connect(&relay).await;
        assert_eq!(
            exchange(&mut client, &request(8, 1, 1)).await.pdu(),
            &[0x83, 11]
        );
        assert_eq!(
            exchange(&mut client, &request(8, 1, 2)).await.pdu(),
            &[3, 2, 0, 2]
        );
        assert_eq!(relay.metrics.devices[0].1.protocol_errors.load(Relaxed), 1);
        backend.finish().await;
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn late_response_after_timeout_never_reaches_next_client() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["timeout"] = json!(0.03);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut first, false).await.unwrap().0;
        let (mut second, _) = listener.accept().await.unwrap();
        let _ = first.write_all(response(&req).as_bytes()).await;
        let req = Frame::read(&mut second, false).await.unwrap().0;
        second.write_all(response(&req).as_bytes()).await.unwrap();
    });
    let mut a = connect(&relay).await;
    let mut b = connect(&relay).await;
    assert_eq!(exchange(&mut a, &request(1, 1, 1)).await.pdu(), &[0x83, 11]);
    assert_eq!(
        exchange(&mut b, &request(1, 1, 2)).await.pdu(),
        &[3, 2, 0, 2]
    );
    assert_eq!(relay.metrics.devices[0].1.timeouts.load(Relaxed), 1);
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn exception_lengths_correct_short_padded_and_delayed_padding() {
    for (announced, padding) in [(3, 0), (4, 0), (4, 1), (4, 2)] {
        let listener = bound().await;
        let relay = start(config(listener.local_addr().unwrap())).await;
        let backend = spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = Frame::read(&mut stream, false).await.unwrap().0;
            let mut bytes = req.exception(2).as_bytes().to_vec();
            bytes[5] = announced;
            if padding == 1 {
                bytes.push(0);
            }
            stream.write_all(&bytes).await.unwrap();
            if padding == 2 {
                sleep(Duration::from_millis(10)).await;
                let _ = stream.write_all(&[0]).await;
            }
            if announced != 3 {
                stream = listener.accept().await.unwrap().0;
            }
            let req = Frame::read(&mut stream, false).await.unwrap().0;
            stream.write_all(response(&req).as_bytes()).await.unwrap();
        });
        let mut client = connect(&relay).await;
        let start = Instant::now();
        let reply = exchange(&mut client, &request(88, 1, 1)).await;
        assert_eq!(reply.as_bytes(), &[0, 88, 0, 0, 0, 3, 1, 0x83, 2]);
        assert!(start.elapsed() < Duration::from_millis(200));
        assert_eq!(
            exchange(&mut client, &request(89, 1, 3)).await.pdu(),
            &[3, 2, 0, 3]
        );
        assert_eq!(
            relay.metrics.devices[0].1.repaired_exceptions.load(Relaxed),
            u64::from(announced != 3)
        );
        backend.finish().await;
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn retries_are_configurable_and_writes_are_not_replayed_by_default() {
    for (function, retry_writes, expected) in [(3, false, 2), (6, false, 1), (6, true, 2)] {
        let listener = bound().await;
        let mut cfg = config(listener.local_addr().unwrap());
        cfg["devices"][0]["modbus"]["attempts"] = json!(2);
        cfg["devices"][0]["modbus"]["retry_writes"] = json!(retry_writes);
        let relay = start(cfg).await;
        let calls = Arc::new(AtomicU64::new(0));
        let count = calls.clone();
        let _backend = spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let _ = Frame::read(&mut stream, false).await.unwrap();
                count.fetch_add(1, Relaxed);
            }
        });
        let mut client = connect(&relay).await;
        let req = Frame::new(5, 1, &[function, 0, 0, 0, 1]).unwrap();
        assert_eq!(
            exchange(&mut client, &req).await.pdu(),
            &[function | 0x80, 11]
        );
        assert_eq!(calls.load(Relaxed), expected);
        assert_eq!(
            relay.metrics.devices[0].1.retries.load(Relaxed),
            expected - 1
        );
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn retries_recover_on_fresh_socket_with_backoff() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["attempts"] = json!(2);
    cfg["devices"][0]["modbus"]["reconnect_delay"] = json!(0.04);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = Frame::read(&mut stream, false).await.unwrap();
        drop(stream);
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        stream.write_all(response(&req).as_bytes()).await.unwrap();
    });
    let mut client = connect(&relay).await;
    let start = Instant::now();
    assert_eq!(
        exchange(&mut client, &request(7, 1, 5)).await.pdu(),
        &[3, 2, 0, 5]
    );
    assert!(start.elapsed() >= Duration::from_millis(40));
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn disconnected_client_reply_is_drained_before_next_exchange() {
    let listener = bound().await;
    let relay = start(config(listener.local_addr().unwrap())).await;
    let seen = Arc::new(Notify::new());
    let notified = seen.clone();
    let release = Arc::new(Notify::new());
    let go = release.clone();
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        notified.notify_one();
        go.notified().await;
        stream.write_all(response(&req).as_bytes()).await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        stream.write_all(response(&req).as_bytes()).await.unwrap();
    });
    let mut a = connect(&relay).await;
    send(&mut a, &request(1, 1, 1)).await;
    timeout(Duration::from_secs(2), seen.notified())
        .await
        .unwrap();
    drop(a);
    let mut b = connect(&relay).await;
    send(&mut b, &request(1, 1, 2)).await;
    release.notify_one();
    assert_eq!(receive(&mut b).await.pdu(), &[3, 2, 0, 2]);
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn bounded_fifo_queue_rejects_overload_and_expires_without_sending() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["queue_capacity"] = json!(1);
    cfg["devices"][0]["queue_timeout"] = json!(0.02);
    let relay = start(cfg).await;
    let release = Arc::new(Notify::new());
    let go = release.clone();
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        go.notified().await;
        stream.write_all(response(&req).as_bytes()).await.unwrap();
        // No second request may have reached the device.
        let mut b = [0];
        assert!(
            timeout(Duration::from_millis(60), stream.read(&mut b))
                .await
                .is_err()
        );
    });
    let mut a = connect(&relay).await;
    send(&mut a, &request(1, 1, 1)).await;
    wait_for(|| relay.metrics.devices[0].1.bytes_sent.load(Relaxed) > 0).await;
    let mut b = connect(&relay).await;
    send(&mut b, &Frame::new(2, 1, &[6, 0, 0, 0, 2]).unwrap()).await;
    wait_for(|| relay.metrics.devices[0].1.queued.load(Relaxed) == 1).await;
    let mut c = connect(&relay).await;
    assert_eq!(exchange(&mut c, &request(3, 1, 3)).await.pdu(), &[0x83, 6]);
    sleep(Duration::from_millis(30)).await;
    release.notify_one();
    assert_eq!(receive(&mut a).await.pdu(), &[3, 2, 0, 1]);
    assert_eq!(receive(&mut b).await.pdu(), &[0x86, 6]);
    assert_eq!(relay.metrics.devices[0].1.busy.load(Relaxed), 1);
    assert_eq!(relay.metrics.devices[0].1.expired.load(Relaxed), 1);
    assert_eq!(relay.metrics.devices[0].1.queued.load(Relaxed), 0);
    sleep(Duration::from_millis(70)).await;
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn fifo_order_and_no_overlapping_requests() {
    let listener = bound().await;
    let relay = start(config(listener.local_addr().unwrap())).await;
    let release = Arc::new(Notify::new());
    let go = release.clone();
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        for n in 1..=3 {
            let req = Frame::read(&mut stream, false).await.unwrap().0;
            assert_eq!(req.pdu()[2], n);
            if n == 1 {
                go.notified().await;
            }
            let mut byte = [0];
            assert!(
                matches!(stream.try_read(&mut byte),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock)
            );
            stream.write_all(response(&req).as_bytes()).await.unwrap();
        }
    });
    let mut clients = vec![];
    for n in 1..=3 {
        let mut client = connect(&relay).await;
        send(&mut client, &request(n, 1, n)).await;
        clients.push(client);
        if n == 1 {
            wait_for(|| relay.metrics.devices[0].1.bytes_sent.load(Relaxed) > 0).await;
        } else {
            wait_for(|| relay.metrics.devices[0].1.queued.load(Relaxed) == u64::from(n - 1)).await;
        }
    }
    release.notify_one();
    for (i, client) in clients.iter_mut().enumerate() {
        assert_eq!(receive(client).await.pdu()[3], (i + 1) as u8);
    }
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn client_limits_and_partial_frames_do_not_block_other_clients() {
    let (addr, _, _task) = echo().await;
    let mut cfg = config(addr);
    cfg["devices"][0]["max_clients"] = json!(2);
    cfg["devices"][0]["frame_timeout"] = json!(0.03);
    let relay = start(cfg).await;
    let mut slow = connect(&relay).await;
    slow.write_all(&[0]).await.unwrap();
    let mut good = connect(&relay).await;
    wait_for(|| relay.metrics.devices[0].1.clients.load(Relaxed) == 2).await;
    let mut rejected = connect(&relay).await;
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(1), rejected.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(
        exchange(&mut good, &request(1, 1, 7)).await.pdu(),
        &[3, 2, 0, 7]
    );
    assert_eq!(
        timeout(Duration::from_secs(1), slow.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(relay.metrics.devices[0].1.rejected_clients.load(Relaxed), 1);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_clients_are_closed_without_touching_backend() {
    let (addr, count, _task) = echo().await;
    let relay = start(config(addr)).await;
    for bytes in [
        &[0, 1, 0, 1, 0, 2, 1, 3][..],
        &[0, 1, 0, 0, 255, 255, 1, 3],
        &[0, 1, 0, 0, 0, 2, 1, 0x83],
    ] {
        let mut client = connect(&relay).await;
        client.write_all(bytes).await.unwrap();
        let mut b = [0];
        assert_eq!(
            timeout(Duration::from_secs(1), client.read(&mut b))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
    assert_eq!(count.load(Relaxed), 0);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn idle_and_ttl_release_backend_and_reconnect() {
    for setting in ["idle_time", "connection_ttl"] {
        let (addr, count, _task) = echo().await;
        let mut cfg = config(addr);
        cfg["devices"][0]["modbus"][setting] = json!(0.03);
        let relay = start(cfg).await;
        let mut client = connect(&relay).await;
        exchange(&mut client, &request(1, 1, 1)).await;
        wait_for(|| !relay.metrics.devices[0].1.connected.load(Relaxed)).await;
        exchange(&mut client, &request(2, 1, 2)).await;
        assert_eq!(count.load(Relaxed), 2);
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn ttl_never_interrupts_active_exchange_and_pacing_is_enforced() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["connection_ttl"] = json!(0.01);
    cfg["devices"][0]["modbus"]["connection_time"] = json!(0.02);
    cfg["devices"][0]["modbus"]["request_delay"] = json!(0.03);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = Frame::read(&mut stream, false).await.unwrap().0;
            sleep(Duration::from_millis(20)).await;
            stream.write_all(response(&req).as_bytes()).await.unwrap();
        }
    });
    let mut client = connect(&relay).await;
    let t = Instant::now();
    exchange(&mut client, &request(1, 1, 1)).await;
    assert!(t.elapsed() >= Duration::from_millis(40));
    let t = Instant::now();
    exchange(&mut client, &request(2, 1, 2)).await;
    assert!(t.elapsed() >= Duration::from_millis(65));
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn monitoring_uses_response_freshness_even_after_backend_failure() {
    let (addr, _, backend) = echo().await;
    let relay = start(config(addr)).await;
    assert!(http(&relay, "/healthz").await.starts_with("HTTP/1.1 200"));
    assert!(http(&relay, "/readyz").await.starts_with("HTTP/1.1 200"));
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 503"));
    let mut client = connect(&relay).await;
    exchange(&mut client, &request(1, 1, 2)).await;
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 200"));
    let metrics = http(&relay, "/metrics").await;
    assert!(metrics.contains("modbus_relay_requests_total{device=\"plc\"} 1\n"));
    assert!(metrics.contains("modbus_relay_request_duration_seconds_count{device=\"plc\"} 1\n"));
    assert!(metrics.contains("# TYPE modbus_relay_request_duration_seconds histogram"));
    // Advance the freshness window without real sleeps or timers racing TCP I/O.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 503"));
    assert!(
        http(&relay, "/metrics")
            .await
            .contains("modbus_relay_backend_healthy{device=\"plc\"} 0\n")
    );
    assert!(http(&relay, "/healthz").await.starts_with("HTTP/1.1 200"));
    exchange(&mut client, &request(2, 1, 2)).await;
    let last = relay.metrics.devices[0].1.last_success_ms.load(Relaxed);
    drop(backend);
    assert_eq!(
        exchange(&mut client, &request(3, 1, 2)).await.pdu(),
        &[0x83, 11]
    );
    assert_eq!(
        relay.metrics.devices[0].1.last_success_ms.load(Relaxed),
        last
    );
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 200"));
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 503"));
    assert!(http(&relay, "/healthz").await.starts_with("HTTP/1.1 200"));
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn disconnecting_on_bad_register_and_retry_does_not_flap_health() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["attempts"] = json!(2);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let good = receive(&mut first).await;
        send(&mut first, &response(&good)).await;
        // Model the reported device behavior: close for this address, including
        // on the retry, rather than returning an Illegal Data Address exception.
        assert_eq!(receive(&mut first).await.pdu(), &[3, 0xff, 0xff, 0, 1]);
        drop(first);
        let (mut retry, _) = listener.accept().await.unwrap();
        assert_eq!(receive(&mut retry).await.pdu(), &[3, 0xff, 0xff, 0, 1]);
        drop(retry);
        let (mut recovered, _) = listener.accept().await.unwrap();
        let good = receive(&mut recovered).await;
        send(&mut recovered, &response(&good)).await;
        // The test advances the health clock by 31 s while this read is idle.
        let bad = Frame::read(&mut recovered, false).await.unwrap().0;
        send(&mut recovered, &bad.exception(2)).await;
    });
    let mut client = connect(&relay).await;
    exchange(&mut client, &request(1, 1, 2)).await;
    let stats = &relay.metrics.devices[0].1;
    let last = stats.last_success_ms.load(Relaxed);
    assert_eq!(
        exchange(&mut client, &request(2, 1, 0xffff)).await.pdu(),
        &[0x83, 11]
    );
    assert_eq!(stats.last_success_ms.load(Relaxed), last);
    assert_eq!(stats.failures.load(Relaxed), 2);
    assert_eq!(stats.retries.load(Relaxed), 1);
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 200"));
    let metrics = http(&relay, "/metrics").await;
    assert!(metrics.contains("modbus_relay_backend_healthy{device=\"plc\"} 1\n"));
    assert!(metrics.contains("modbus_relay_failures_total{device=\"plc\"} 2\n"));
    assert_eq!(
        exchange(&mut client, &request(3, 1, 2)).await.pdu(),
        &[3, 2, 0, 2]
    );

    // No polling turns health red. A valid device exception then refreshes it.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 503"));
    assert_eq!(
        exchange(&mut client, &request(4, 1, 42)).await.pdu(),
        &[0x83, 2]
    );
    assert_eq!(stats.exceptions.load(Relaxed), 1);
    assert_eq!(stats.responses.load(Relaxed), 3);
    assert!(http(&relay, "/health").await.starts_with("HTTP/1.1 200"));
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn management_http_methods_limits_and_malformed_input() {
    let (addr, _, _task) = echo().await;
    let relay = start(config(addr)).await;
    assert!(http(&relay, "/missing").await.starts_with("HTTP/1.1 404"));
    assert!(
        raw_http(&relay, "POST /metrics HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .starts_with("HTTP/1.1 405")
    );
    assert!(
        raw_http(
            &relay,
            "GET /metrics HTTP/1.1\r\nContent-Length: 1\r\n\r\nx"
        )
        .await
        .starts_with("HTTP/1.1 400")
    );
    let head = raw_http(&relay, "HEAD /healthz HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(head.ends_with("\r\n\r\n"));
    let huge = format!("GET / HTTP/1.1\r\nX: {}", "x".repeat(4077));
    assert!(raw_http(&relay, &huge).await.starts_with("HTTP/1.1 431"));
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn multiple_listeners_share_backend_and_devices_are_independent() {
    let (addr, count, _task) = echo().await;
    let (other, _, _second) = echo().await;
    let mut cfg = config(addr);
    cfg["devices"][0].as_object_mut().unwrap().remove("listen");
    cfg["devices"][0]["listeners"] = json!([{"bind":"127.0.0.1:0"},{"bind":"127.0.0.1:0"}]);
    let mut second = config(other)["devices"][0].clone();
    second["name"] = json!("second");
    cfg["devices"].as_array_mut().unwrap().push(second);
    let relay = start(cfg).await;
    for addr in relay.listeners.iter().flatten() {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        assert_eq!(
            exchange(&mut stream, &request(4, 1, 2)).await.pdu(),
            &[3, 2, 0, 2]
        );
    }
    assert_eq!(count.load(Relaxed), 1);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn stalled_device_does_not_block_a_different_device_or_monitoring() {
    let stalled = bound().await;
    let (fast, _, _echo) = echo().await;
    let mut cfg = config(stalled.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["timeout"] = json!(0.3);
    let mut second = config(fast)["devices"][0].clone();
    second["name"] = json!("fast");
    cfg["devices"].as_array_mut().unwrap().push(second);
    let relay = start(cfg).await;
    let mut slow = connect(&relay).await;
    send(&mut slow, &request(1, 1, 1)).await;
    wait_for(|| relay.metrics.devices[0].1.bytes_sent.load(Relaxed) > 0).await;
    let mut quick = TcpStream::connect(relay.listeners[1][0]).await.unwrap();
    let reply = timeout(
        Duration::from_millis(200),
        exchange(&mut quick, &request(2, 1, 9)),
    )
    .await
    .unwrap();
    assert_eq!(reply.pdu(), &[3, 2, 0, 9]);
    assert!(http(&relay, "/healthz").await.starts_with("HTTP/1.1 200"));
    assert_eq!(receive(&mut slow).await.pdu(), &[0x83, 11]);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn idle_clients_release_their_slot() {
    let (addr, _, _backend) = echo().await;
    let mut cfg = config(addr);
    cfg["devices"][0]["max_clients"] = json!(1);
    cfg["devices"][0]["client_idle_timeout"] = json!(0.02);
    let relay = start(cfg).await;
    let mut idle = connect(&relay).await;
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(1), idle.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    wait_for(|| relay.metrics.devices[0].1.clients.load(Relaxed) == 0).await;
    let mut client = connect(&relay).await;
    assert_eq!(
        exchange(&mut client, &request(1, 1, 1)).await.pdu(),
        &[3, 2, 0, 1]
    );
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn monitoring_connection_limit_prevents_unbounded_http_tasks() {
    let (addr, _, _backend) = echo().await;
    let mut cfg = config(addr);
    cfg["monitoring"]["max_connections"] = json!(1);
    let relay = start(cfg).await;
    let mut slow = TcpStream::connect(relay.monitoring).await.unwrap();
    slow.write_all(b"GET /healthz HTTP/1.1\r\n").await.unwrap();
    // The incomplete request occupies the sole slot. FIFO accept order ensures
    // the second TCP connection cannot consume it first.
    let mut rejected = TcpStream::connect(relay.monitoring).await.unwrap();
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(1), rejected.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    slow.write_all(b"Host: test\r\n\r\n").await.unwrap();
    let mut body = String::new();
    timeout(Duration::from_secs(1), slow.read_to_string(&mut body))
        .await
        .unwrap()
        .unwrap();
    assert!(body.starts_with("HTTP/1.1 200"));
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn rtu_echo_mismatch_is_rejected() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["mode"] = json!("rtuovertcp");
    cfg["devices"][0]["modbus"]["strip_rtu_echo"] = json!(true);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8];
        stream.read_exact(&mut bytes).await.unwrap();
        bytes[0] ^= 1;
        stream.write_all(&bytes).await.unwrap();
    });
    let mut client = connect(&relay).await;
    assert_eq!(
        exchange(&mut client, &request(1, 1, 1)).await.pdu(),
        &[0x83, 11]
    );
    backend.finish().await;
    assert_eq!(relay.metrics.devices[0].1.protocol_errors.load(Relaxed), 1);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_start_releases_all_previously_bound_listeners() {
    let free = bound().await;
    let free_addr = free.local_addr().unwrap();
    drop(free);
    let occupied = bound().await;
    let mut cfg = config("127.0.0.1:502".parse().unwrap());
    cfg["devices"][0]["listen"]["bind"] = json!(free_addr.to_string());
    cfg["monitoring"]["bind"] = json!(occupied.local_addr().unwrap().to_string());
    assert!(
        Relay::start(serde_json::from_value(cfg).unwrap())
            .await
            .is_err()
    );
    TcpListener::bind(free_addr).await.unwrap();
}

#[tokio::test]
async fn shutdown_drains_active_reply_and_releases_sockets_and_gauges() {
    let listener = bound().await;
    let relay = start(config(listener.local_addr().unwrap())).await;
    let release = Arc::new(Notify::new());
    let go = release.clone();
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        go.notified().await;
        stream.write_all(response(&req).as_bytes()).await.unwrap();
    });
    let mut client = connect(&relay).await;
    send(&mut client, &request(1, 1, 1)).await;
    wait_for(|| relay.metrics.devices[0].1.bytes_sent.load(Relaxed) > 0).await;
    let addr = relay.listeners[0][0];
    let monitoring = relay.monitoring;
    let metrics = relay.metrics.clone();
    let stop = tokio::spawn(relay.shutdown());
    wait_for(|| !metrics.ready.load(Relaxed)).await;
    // The accept socket must close during draining, before the backend replies.
    timeout(Duration::from_millis(150), async {
        while TcpStream::connect(addr).await.is_ok() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("listener still open during graceful shutdown");
    assert!(!stop.is_finished());
    release.notify_one();
    assert_eq!(receive(&mut client).await.pdu(), &[3, 2, 0, 1]);
    backend.finish().await;
    timeout(Duration::from_secs(1), stop)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(metrics.devices[0].1.clients.load(Relaxed), 0);
    assert_eq!(metrics.devices[0].1.queued.load(Relaxed), 0);
    assert!(!metrics.devices[0].1.connected.load(Relaxed));
    assert!(TcpStream::connect(addr).await.is_err());
    assert!(TcpStream::connect(monitoring).await.is_err());
}

#[tokio::test]
async fn shutdown_deadline_cancels_partial_backend_frame_and_pending_queue() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["modbus"]["timeout"] = json!(60);
    let relay = start(cfg).await;
    let mut active = connect(&relay).await;
    send(&mut active, &request(1, 1, 1)).await;
    let (mut backend, _) = listener.accept().await.unwrap();
    let req = Frame::read(&mut backend, false).await.unwrap().0;
    backend
        .write_all(&response(&req).as_bytes()[..7])
        .await
        .unwrap();
    let mut waiting = connect(&relay).await;
    send(&mut waiting, &request(2, 1, 2)).await;
    wait_for(|| relay.metrics.devices[0].1.queued.load(Relaxed) == 1).await;
    let metrics = relay.metrics.clone();
    tokio::time::pause();
    let shutdown = tokio::spawn(relay.shutdown());
    wait_for(|| !metrics.ready.load(Relaxed)).await;
    tokio::time::advance(Duration::from_secs(6)).await;
    timeout(Duration::from_secs(1), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // JoinSet children must also have released their sockets and accounting.
    wait_for(|| metrics.devices[0].1.clients.load(Relaxed) == 0).await;
    assert_eq!(metrics.devices[0].1.queued.load(Relaxed), 0);
    assert!(!metrics.devices[0].1.connected.load(Relaxed));
    // FIN is delivered by the real kernel; do not auto-advance a virtual timeout
    // while waiting for readiness from a real socket.
    tokio::time::resume();
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(1), backend.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn rtu_transport_echo_crc_errors_and_unsupported_functions() {
    for (echo, corrupt) in [(false, false), (true, false), (true, true)] {
        let listener = bound().await;
        let mut cfg = config(listener.local_addr().unwrap());
        cfg["devices"][0]["modbus"]["mode"] = json!("rtuovertcp");
        cfg["devices"][0]["modbus"]["strip_rtu_echo"] = json!(echo);
        let relay = start(cfg).await;
        let backend = spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 8];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(modbus_relay::frame::crc16(&bytes), 0);
            if echo {
                stream.write_all(&bytes).await.unwrap();
            }
            let (mut bytes, len) = rtu_request(&Frame::new(0, 1, &[3, 2, 0, 9]).unwrap());
            if corrupt {
                bytes[len - 1] ^= 1;
            }
            stream.write_all(&bytes[..len]).await.unwrap();
        });
        let mut client = connect(&relay).await;
        assert_eq!(
            exchange(&mut client, &Frame::new(1, 1, &[65, 0]).unwrap())
                .await
                .pdu(),
            &[0xc1, 1]
        );
        assert_eq!(
            exchange(&mut client, &request(7, 0, 1)).await.pdu(),
            &[0x83, 1]
        );
        let reply = exchange(&mut client, &request(8, 1, 9)).await;
        assert_eq!(reply.tid(), 8);
        assert_eq!(
            reply.pdu(),
            if corrupt {
                &[0x83, 11][..]
            } else {
                &[3, 2, 0, 9][..]
            }
        );
        backend.finish().await;
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn invalid_standard_requests_never_reach_backend_and_pipeline_recovers() {
    let (addr, count, _task) = echo().await;
    let relay = start(config(addr)).await;
    let mut client = connect(&relay).await;
    for (pdu, code) in [
        (vec![15, 0, 0, 0, 9, 1, 0], 3),
        (vec![15, 0, 0, 0, 8, 2, 0, 0], 3),
        (vec![16, 0, 0, 0, 1, 1, 0], 3),
        (vec![5, 0, 0, 0, 1], 3),
        (vec![3, 255, 255, 0, 2], 2),
    ] {
        let req = Frame::new(17, 1, &pdu).unwrap();
        assert_eq!(
            exchange(&mut client, &req).await.as_bytes(),
            req.exception(code).as_bytes()
        );
    }
    assert_eq!(count.load(Relaxed), 0);
    assert_eq!(relay.metrics.devices[0].1.invalid_requests.load(Relaxed), 5);
    let req = request(18, 1, 7);
    assert_eq!(
        exchange(&mut client, &req).await.as_bytes(),
        response(&req).as_bytes()
    );
    assert_eq!(count.load(Relaxed), 1);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_confirmations_reset_socket_and_vendor_exceptions_survive() {
    for (pdu, bad) in [
        (vec![22, 0, 1, 0xff, 0, 0, 7], vec![22, 0, 1, 0xff, 0, 0, 8]),
        (vec![6, 0, 1, 0, 7], vec![6, 0, 2, 0, 7]),
        (vec![16, 0, 1, 0, 1, 2, 0, 7], vec![16, 0, 1, 0, 2]),
        (vec![3, 0, 0, 0, 1], vec![3, 1, 0]),
        (vec![0x41, 0], vec![0xc1, 0]),
    ] {
        let listener = bound().await;
        let relay = start(config(listener.local_addr().unwrap())).await;
        let backend = spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let req = Frame::read(&mut first, false).await.unwrap().0;
            send(
                &mut first,
                &Frame::new(req.tid(), req.unit(), &bad).unwrap(),
            )
            .await;
            let mut byte = [0];
            assert_eq!(first.read(&mut byte).await.unwrap(), 0);
            let (mut second, _) = listener.accept().await.unwrap();
            let req = Frame::read(&mut second, false).await.unwrap().0;
            send(&mut second, &req.exception(0x80)).await;
        });
        let mut client = connect(&relay).await;
        let req = Frame::new(42, 1, &pdu).unwrap();
        assert_eq!(
            exchange(&mut client, &req).await.as_bytes(),
            req.exception(11).as_bytes()
        );
        assert_eq!(
            exchange(&mut client, &req).await.as_bytes(),
            req.exception(0x80).as_bytes()
        );
        assert_eq!(relay.metrics.devices[0].1.protocol_errors.load(Relaxed), 1);
        backend.finish().await;
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn total_deadline_covers_connection_warmup_without_sending_write() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["request_timeout"] = json!(0.05);
    cfg["devices"][0]["modbus"]["connection_time"] = json!(0.25);
    let relay = start(cfg).await;
    let mut client = connect(&relay).await;
    let req = Frame::new(1, 1, &[6, 0, 0, 0, 1]).unwrap();
    send(&mut client, &req).await;
    let (mut backend, _) = listener.accept().await.unwrap();
    assert_eq!(
        receive(&mut client).await.as_bytes(),
        req.exception(11).as_bytes()
    );
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(1), backend.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(relay.metrics.devices[0].1.bytes_sent.load(Relaxed), 0);
    assert_eq!(
        relay.metrics.devices[0].1.request_deadlines.load(Relaxed),
        1
    );
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn total_deadline_covers_retry_backoff_and_never_reconnects_late() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["request_timeout"] = json!(0.06);
    cfg["devices"][0]["modbus"]["attempts"] = json!(3);
    cfg["devices"][0]["modbus"]["reconnect_delay"] = json!(0.2);
    let relay = start(cfg).await;
    let mut client = connect(&relay).await;
    send(&mut client, &request(1, 1, 1)).await;
    let (mut backend, _) = listener.accept().await.unwrap();
    let req = Frame::read(&mut backend, false).await.unwrap().0;
    let mut bad = response(&req);
    bad.set_tid(req.tid().wrapping_add(1));
    send(&mut backend, &bad).await;
    assert_eq!(receive(&mut client).await.pdu(), &[0x83, 11]);
    assert!(
        timeout(Duration::from_millis(230), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(relay.metrics.devices[0].1.connections.load(Relaxed), 1);
    assert_eq!(
        relay.metrics.devices[0].1.request_deadlines.load(Relaxed),
        1
    );
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn total_deadline_expires_queued_write_during_pacing_without_late_execution() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["request_timeout"] = json!(0.08);
    cfg["devices"][0]["modbus"]["request_delay"] = json!(0.2);
    let relay = start(cfg).await;
    let mut active = connect(&relay).await;
    send(&mut active, &request(1, 1, 1)).await;
    let (mut backend, _) = listener.accept().await.unwrap();
    let _ = Frame::read(&mut backend, false).await.unwrap();
    let mut queued = connect(&relay).await;
    let write = Frame::new(2, 1, &[6, 0, 0, 0, 99]).unwrap();
    send(&mut queued, &write).await;
    wait_for(|| relay.metrics.devices[0].1.queued.load(Relaxed) == 1).await;
    assert_eq!(receive(&mut active).await.pdu(), &[0x83, 11]);
    assert_eq!(receive(&mut queued).await.pdu(), &[0x86, 11]);
    assert!(
        timeout(Duration::from_millis(230), listener.accept())
            .await
            .is_err()
    );
    let mut byte = [0];
    assert_eq!(backend.read(&mut byte).await.unwrap(), 0);
    assert_eq!(
        relay.metrics.devices[0].1.request_deadlines.load(Relaxed),
        2
    );
    assert_eq!(relay.metrics.devices[0].1.bytes_sent.load(Relaxed), 12);
    assert_eq!(relay.metrics.devices[0].1.queued.load(Relaxed), 0);
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn total_deadline_cancels_partial_frame_and_next_request_uses_fresh_socket() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["request_timeout"] = json!(0.06);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut first, false).await.unwrap().0;
        first
            .write_all(&response(&req).as_bytes()[..8])
            .await
            .unwrap();
        let mut byte = [0];
        assert_eq!(first.read(&mut byte).await.unwrap(), 0);
        let (mut next, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut next, false).await.unwrap().0;
        send(&mut next, &response(&req)).await;
    });
    let mut client = connect(&relay).await;
    assert_eq!(
        exchange(&mut client, &request(1, 1, 1)).await.pdu(),
        &[0x83, 11]
    );
    assert_eq!(
        exchange(&mut client, &request(1, 1, 2)).await.pdu(),
        &[3, 2, 0, 2]
    );
    assert_eq!(
        relay.metrics.devices[0].1.request_deadlines.load(Relaxed),
        1
    );
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn slow_backend_gets_a_fresh_downstream_write_deadline() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["frame_timeout"] = json!(0.02);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        sleep(Duration::from_millis(80)).await;
        send(&mut stream, &response(&req)).await;
    });
    let mut client = connect(&relay).await;
    assert_eq!(
        exchange(&mut client, &request(1, 1, 42)).await.pdu(),
        &[3, 2, 0, 42]
    );
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn health_scrapes_do_not_create_extra_backend_connections() {
    let (addr, count, _task) = echo().await;
    let relay = start(config(addr)).await;
    for expected in [0, 1] {
        for _ in 0..20 {
            assert!(http(&relay, "/health").await.starts_with(if expected == 0 {
                "HTTP/1.1 503"
            } else {
                "HTTP/1.1 200"
            }));
            assert!(http(&relay, "/healthz").await.starts_with("HTTP/1.1 200"));
            assert!(http(&relay, "/metrics").await.starts_with("HTTP/1.1 200"));
        }
        assert_eq!(count.load(Relaxed), expected);
        if expected == 0 {
            let mut client = connect(&relay).await;
            assert_eq!(
                exchange(&mut client, &request(1, 1, 1)).await.pdu(),
                &[3, 2, 0, 1]
            );
        }
    }
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn maximum_fc23_frames_are_not_truncated_in_tcp_or_rtu() {
    for mode in ["tcp", "rtuovertcp"] {
        let listener = bound().await;
        let mut cfg = config(listener.local_addr().unwrap());
        cfg["devices"][0]["modbus"]["mode"] = json!(mode);
        let relay = start(cfg).await;
        let mut pdu = vec![23, 0, 0, 0, 125, 0, 0, 0, 121, 242];
        pdu.resize(252, 0x33);
        let request = Frame::new(7, 1, &pdu).unwrap();
        let mut reply = vec![23, 250];
        reply.extend((0..250).map(|n| n as u8));
        let expected = Frame::new(7, 1, &reply).unwrap();
        let input = request.clone();
        let output = expected.clone();
        let backend = spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.set_nodelay(true).unwrap();
            let bytes = if mode == "tcp" {
                let req = Frame::read(&mut stream, false).await.unwrap().0;
                assert_eq!(req.pdu(), input.pdu());
                Frame::new(req.tid(), req.unit(), output.pdu())
                    .unwrap()
                    .as_bytes()
                    .to_vec()
            } else {
                let (expected, len) = rtu_request(&input);
                let mut received = vec![0; len];
                stream.read_exact(&mut received).await.unwrap();
                assert_eq!(received, &expected[..len]);
                let (bytes, len) = rtu_request(&output);
                bytes[..len].to_vec()
            };
            for chunk in bytes.chunks(3) {
                stream.write_all(chunk).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let mut client = connect(&relay).await;
        assert_eq!(
            exchange(&mut client, &request).await.as_bytes(),
            expected.as_bytes()
        );
        backend.finish().await;
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn rtu_fc6_echo_requires_separate_ack_and_echo_only_times_out() {
    for (strip_echo, copies, success) in [(false, 1, true), (true, 2, true), (true, 1, false)] {
        let listener = bound().await;
        let mut cfg = config(listener.local_addr().unwrap());
        cfg["devices"][0]["modbus"]["mode"] = json!("rtuovertcp");
        cfg["devices"][0]["modbus"]["strip_rtu_echo"] = json!(strip_echo);
        cfg["devices"][0]["modbus"]["timeout"] = json!(0.05);
        let relay = start(cfg).await;
        let backend = spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.set_nodelay(true).unwrap();
            let mut request = [0; 8];
            stream.read_exact(&mut request).await.unwrap();
            for _ in 0..copies {
                for byte in request {
                    stream.write_all(&[byte]).await.unwrap();
                }
            }
            if !success {
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
            }
        });
        let mut client = connect(&relay).await;
        let req = Frame::new(1, 1, &[6, 0, 1, 0, 7]).unwrap();
        assert_eq!(
            exchange(&mut client, &req).await.as_bytes(),
            if success {
                req.clone()
            } else {
                req.exception(11)
            }
            .as_bytes()
        );
        backend.finish().await;
        relay.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn read_write_read_observes_device_state_without_stale_cache() {
    let listener = bound().await;
    let relay = start(config(listener.local_addr().unwrap())).await;
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut value = 7;
        for _ in 0..3 {
            let req = Frame::read(&mut stream, false).await.unwrap().0;
            let reply = if req.function() == 6 {
                value = req.pdu()[4];
                req
            } else {
                Frame::new(req.tid(), req.unit(), &[3, 2, 0, value]).unwrap()
            };
            send(&mut stream, &reply).await;
        }
    });
    let mut client = connect(&relay).await;
    assert_eq!(
        exchange(&mut client, &request(1, 1, 0)).await.pdu(),
        &[3, 2, 0, 7]
    );
    let write = Frame::new(2, 1, &[6, 0, 0, 0, 42]).unwrap();
    assert_eq!(exchange(&mut client, &write).await.pdu(), write.pdu());
    assert_eq!(
        exchange(&mut client, &request(3, 1, 0)).await.pdu(),
        &[3, 2, 0, 42]
    );
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn trickled_bytes_do_not_extend_whole_frame_deadlines() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["frame_timeout"] = json!(0.035);
    cfg["devices"][0]["modbus"]["timeout"] = json!(0.035);
    let relay = start(cfg).await;
    let mut client = connect(&relay).await;
    let bytes = request(1, 1, 1).as_bytes().to_vec();
    for byte in &bytes[..4] {
        let _ = client.write_all(&[*byte]).await;
        sleep(Duration::from_millis(15)).await;
    }
    let mut byte = [0];
    assert!(matches!(
        timeout(Duration::from_millis(100), client.read(&mut byte))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    assert_eq!(relay.metrics.devices[0].1.connections.load(Relaxed), 0);
    let backend = spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.set_nodelay(true).unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        for byte in response(&req).as_bytes() {
            if stream.write_all(&[*byte]).await.is_err() {
                break;
            }
            sleep(Duration::from_millis(15)).await;
        }
    });
    let mut client = connect(&relay).await;
    assert_eq!(
        timeout(
            Duration::from_millis(140),
            exchange(&mut client, &request(2, 1, 1))
        )
        .await
        .unwrap()
        .pdu(),
        &[0x83, 11]
    );
    assert_eq!(relay.metrics.devices[0].1.timeouts.load(Relaxed), 1);
    backend.finish().await;
    relay.shutdown().await.unwrap();
}

#[tokio::test]
async fn repeated_disconnects_and_backend_failures_release_resources() {
    let listener = bound().await;
    let mut cfg = config(listener.local_addr().unwrap());
    cfg["devices"][0]["max_clients"] = json!(1);
    let relay = start(cfg).await;
    let backend = spawn(async move {
        for _ in 0..100 {
            let (mut stream, _) = listener.accept().await.unwrap();
            Frame::read(&mut stream, false).await.unwrap();
            // A device-side disconnect must not stop the listener or retain slots.
        }
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = Frame::read(&mut stream, false).await.unwrap().0;
        send(&mut stream, &response(&req)).await;
    });
    for _ in 0..100 {
        let mut client = connect(&relay).await;
        assert_eq!(
            exchange(&mut client, &request(1, 1, 1)).await.pdu(),
            &[0x83, 11]
        );
        drop(client);
        wait_for(|| relay.metrics.devices[0].1.clients.load(Relaxed) == 0).await;
        assert_eq!(relay.metrics.devices[0].1.queued.load(Relaxed), 0);
        assert!(!relay.metrics.devices[0].1.connected.load(Relaxed));
    }
    let mut client = connect(&relay).await;
    assert_eq!(
        exchange(&mut client, &request(1, 1, 42)).await.pdu(),
        &[3, 2, 0, 42]
    );
    assert_eq!(relay.metrics.devices[0].1.failures.load(Relaxed), 100);
    assert_eq!(relay.metrics.devices[0].1.connections.load(Relaxed), 101);
    backend.finish().await;
    relay.shutdown().await.unwrap();
}
