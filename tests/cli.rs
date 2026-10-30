use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_modbus-relay"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_version_and_invalid_arguments() {
    assert!(run(&["--help"]).status.success());
    assert!(
        String::from_utf8(run(&["--version"]).stdout)
            .unwrap()
            .starts_with("modbus-relay 0.1.0")
    );
    for args in [
        &[][..],
        &["--modbus", "host:502", "--attempts", "0", "--check"],
        &["--workers", "0"],
        &["--bind", "0:502"],
        &["--modbus", "host:502", "--timeout", "NaN", "--check"],
    ] {
        assert!(!run(args).status.success(), "{args:?}");
    }
}

#[test]
fn validates_example_configs_and_upstream_cli_without_network() {
    for args in [
        &["--check", "-c", "config.example.yaml"][..],
        &["--check", "-c", "examples/config.toml"],
        &[
            "--check",
            "--modbus",
            "tcp://plc:502",
            "-b",
            "tcp://:5020",
            "--modbus-connection-time",
            "0.1",
        ],
        &[
            "--check",
            "-c",
            "config.example.yaml",
            "--modbus",
            "another:502",
            "-b",
            "0:9001",
        ],
    ] {
        let output = run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("configuration valid"));
    }
}

#[cfg(unix)]
#[test]
fn binary_healthcheck_runtime_and_sigterm() {
    use std::{
        net::{TcpListener, TcpStream},
        process::{Child, Stdio},
        thread::sleep,
        time::{Duration, Instant},
    };
    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for workers in ["1", "2"] {
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let mut child = Guard(
            Command::new(env!("CARGO_BIN_EXE_modbus-relay"))
                .args([
                    "--modbus",
                    "127.0.0.1:1",
                    "--bind",
                    "127.0.0.1:0",
                    "--monitoring-bind",
                    &address.to_string(),
                    "--workers",
                    workers,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let start = Instant::now();
        while TcpStream::connect(address).is_err() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "binary did not start"
            );
            assert!(child.0.try_wait().unwrap().is_none());
            sleep(Duration::from_millis(5));
        }
        assert!(
            run(&["--healthcheck", &address.to_string()])
                .status
                .success()
        );
        assert!(
            Command::new("sh")
                .args(["-c", "kill -TERM \"$1\"", "test", &child.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let start = Instant::now();
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "binary did not stop"
            );
            sleep(Duration::from_millis(5));
        }
        assert!(
            !run(&["--healthcheck", &address.to_string()])
                .status
                .success()
        );
    }
}

#[test]
fn logging_target_filters_and_invalid_input_preserve_cli_behavior() {
    struct File(std::path::PathBuf);
    impl Drop for File {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let file = File(
        std::env::temp_dir().join(format!("modbus-relay-log-test-{}.json", std::process::id())),
    );
    let config = |level: &str| {
        serde_json::json!({
            "logging":{"root":{"level":level}},
            "devices":[{"modbus":{"url":"127.0.0.1:502"},"listen":{"bind":"127.0.0.1:0"}}]
        })
        .to_string()
    };
    std::fs::write(&file.0, config("warn")).unwrap();
    for (filter, warning) in [
        ("off", false),
        ("info", true),
        ("off,modbus_relay=warn", true),
        ("off,modbus_relay::proxy=warn", false),
        ("modbus_relay=not-a-level", true),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_modbus-relay"))
            .args(["--check", "-c"])
            .arg(&file.0)
            .env("RUST_LOG", filter)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{filter}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).contains("Python logging config"),
            warning,
            "{filter}"
        );
    }
    std::fs::write(&file.0, config("modbus_relay=not-a-level")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_modbus-relay"))
        .args(["--check", "-c"])
        .arg(&file.0)
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(output.status.success());
}
