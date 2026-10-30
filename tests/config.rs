use modbus_relay::config::{Config, address};
use serde_json::{Value, json};

fn base() -> Value {
    json!({"devices":[{"modbus":{"url":"plc:502"},"listen":{"bind":"0:9000"}}]})
}
fn validate(value: Value) -> Result<Config, modbus_relay::Error> {
    let mut config = Config::parse(&value.to_string(), "json")?;
    config.validate()?;
    Ok(config)
}

#[test]
fn upstream_formats_and_mapping() {
    for (format, text) in [
        (
            "yaml",
            "devices:\n- modbus:\n    url: plc:502\n  listen:\n    bind: 0:9000\n  unit_id_remapping:\n    1: 0\n",
        ),
        (
            "toml",
            "[[devices]]\n[devices.modbus]\nurl='plc:502'\n[devices.listen]\nbind='0:9000'\n[devices.unit_id_remapping]\n'1'=0\n",
        ),
        (
            "json",
            r#"{"devices":[{"modbus":{"url":"plc:502"},"listen":{"bind":"0:9000"},"unit_id_remapping":{"1":0}}]}"#,
        ),
    ] {
        let mut config = Config::parse(text, format).unwrap();
        config.validate().unwrap();
        assert_eq!(config.devices[0].listeners[0].bind, "0.0.0.0:9000");
        assert_eq!(config.devices[0].unit_id_remapping[&1], 0);
        assert_eq!(config.devices[0].modbus.attempts(), 2);
        assert_eq!(config.monitoring.health_max_age, 30.0);
    }
}

#[test]
fn explicit_health_window_is_preserved() {
    let mut cfg = base();
    cfg["monitoring"] = json!({"health_max_age": 60});
    assert_eq!(validate(cfg).unwrap().monitoring.health_max_age, 60.0);
}

#[test]
fn addresses_support_upstream_syntax_dns_ipv6_and_defaults() {
    for (input, expected) in [
        ("tcp://host:502", "host:502"),
        ("host", "host:502"),
        (":502", "0.0.0.0:502"),
        ("tcp://0:123", "0.0.0.0:123"),
        ("[::1]:502", "[::1]:502"),
        ("tcp://[::1]", "[::1]:502"),
    ] {
        assert_eq!(address(input, 502).unwrap(), expected);
    }
    for input in [
        "udp://host:502",
        "host:99999",
        "host:x",
        "user@host:502",
        "host:502/path",
        "::1:502",
        "[xyz]:502",
        "[::1]oops",
        "host name:502",
    ] {
        assert!(address(input, 502).is_err(), "{input}");
    }
}

#[test]
fn invalid_timings_and_limits_are_rejected() {
    for field in [
        "timeout",
        "connection_time",
        "idle_time",
        "connection_ttl",
        "reconnect_delay",
        "request_delay",
    ] {
        for invalid in [-1.0, 86401.0] {
            let mut value = base();
            value["devices"][0]["modbus"][field] = json!(invalid);
            assert!(validate(value).is_err(), "{field}");
        }
    }
    for field in [
        "max_clients",
        "queue_capacity",
        "queue_timeout",
        "request_timeout",
        "client_idle_timeout",
        "frame_timeout",
    ] {
        let mut value = base();
        value["devices"][0][field] = json!(0);
        assert!(validate(value).is_err(), "{field}");
    }
    for invalid in [0, 101] {
        let mut v = base();
        v["devices"][0]["modbus"]["attempts"] = json!(invalid);
        assert!(validate(v).is_err());
    }
    let mut v = base();
    v["devices"][0]["modbus"]["retry_count"] = json!(u32::MAX);
    assert!(validate(v).is_err());
}

#[test]
fn total_budget_preserves_slow_backend_settings_and_validates_override() {
    let mut v = base();
    v["devices"][0]["modbus"]["timeout"] = json!(60);
    let config = validate(v.clone()).unwrap();
    assert_eq!(config.devices[0].request_budget().as_secs(), 250);
    v["devices"][0]["request_timeout"] = json!(0.125);
    assert_eq!(
        validate(v).unwrap().devices[0].request_budget().as_millis(),
        125
    );
    for number in [".nan", ".inf", "-.inf", "-1", "86401"] {
        let text = format!(
            "devices:\n- modbus:\n    url: plc:502\n  listen:\n    bind: 0:9000\n  request_timeout: {number}\n"
        );
        assert!(Config::parse(&text, "yaml").unwrap().validate().is_err());
    }
}

#[test]
fn nonfinite_yaml_timings_rejected() {
    for number in [".nan", ".inf", "-.inf", "0"] {
        let text = format!(
            "devices:\n- modbus:\n    url: plc:502\n    timeout: {number}\n  listen:\n    bind: 0:9000\n"
        );
        assert!(Config::parse(&text, "yaml").unwrap().validate().is_err());
    }
}

#[test]
fn retry_count_alias_and_conflicts() {
    let mut v = base();
    v["devices"][0]["modbus"]["retry_count"] = json!(3);
    assert_eq!(validate(v.clone()).unwrap().devices[0].modbus.attempts(), 4);
    v["devices"][0]["modbus"]["attempts"] = json!(2);
    assert!(validate(v).is_err());
}

#[test]
fn rejects_unknown_fields_and_unsafe_names() {
    let mut v = base();
    v["devices"][0]["modbus"]["timout"] = json!(1);
    assert!(validate(v).is_err());
    for name in ["bad\"label", "hello world", "a\nb", &"x".repeat(65)] {
        let mut v = base();
        v["devices"][0]["name"] = json!(name);
        assert!(validate(v).is_err());
    }
    let mut v = base();
    v["devices"][0]["unit_id_remapping"] = json!({"256":0});
    assert!(validate(v).is_err());
}

#[test]
fn duplicates_and_conflicting_listeners_rejected() {
    let mut v = base();
    let d = v["devices"][0].clone();
    v["devices"].as_array_mut().unwrap().push(d);
    assert!(validate(v).is_err());
    let mut v = base();
    v["devices"][0]["listeners"] = json!([{"bind":"0:9001"}]);
    assert!(validate(v).is_err());
    let mut v = base();
    v["devices"][0].as_object_mut().unwrap().remove("listen");
    assert!(validate(v).is_err());
    assert!(validate(json!({"devices":[]})).is_err());
}

#[test]
fn multiple_listeners_and_validation_idempotent() {
    let mut v = base();
    v["devices"][0].as_object_mut().unwrap().remove("listen");
    v["devices"][0]["listeners"] = json!([{"bind":"0:9000"},{"bind":"0:9001"}]);
    let mut config = validate(v).unwrap();
    config.validate().unwrap();
    assert_eq!(config.devices[0].listeners.len(), 2);
}

#[test]
fn rtu_mode_and_echo_validation() {
    let mut v = base();
    v["devices"][0]["modbus"]["strip_rtu_echo"] = json!(true);
    assert!(validate(v.clone()).is_err());
    v["devices"][0]["modbus"]["mode"] = json!("rtuovertcp");
    assert!(validate(v).is_ok());
}
