# modbus-relay

A small Rust Modbus proxy. Multiple TCP clients share one persistent connection
per device. Requests are serialized; different devices operate independently.
Compatible with the configuration schema of [modbus-proxy](https://github.com/tiagocoutinho/modbus-proxy).

## Run with Docker

```sh
cp config.example.yaml config.yaml
# Set your device address in config.yaml.
MODBUS_RELAY_IMAGE=acidcliff/modbus-relay:0.1.0 docker compose up -d --no-build
```

Prebuilt images for AMD64 and ARM64: [Docker Hub](https://hub.docker.com/r/acidcliff/modbus-relay).
To build locally, use `docker compose up -d --build` instead.

Connect clients to port **5020**. The container runs without root privileges on
`scratch`, with a read-only filesystem and a 32 MiB memory limit by default.

## Monitoring

| Endpoint | Purpose |
|---|---|
| `/healthz` | Process liveness; Docker healthcheck |
| `/readyz` | Accepting requests |
| `/health` | Valid backend response within the last 30 s for every device; suitable for Uptime Kuma |
| `/metrics` | Prometheus metrics |

Monitoring listens on port **8080**. `/health` returns 503 until each device has
answered, then stays green while responses remain within `health_max_age` (default
30 s). Individual failures do not immediately turn it red. Valid Modbus exceptions
count as responses; relay-generated errors do not. Checks are passive: stopped
polling also turns health red. They do not access registers or validate data values.
For Kuma, a name such as **“Modbus: Datenfluss Wechselrichter”** describes this
better than “inverter reachable”. See the [monitoring details](docs/configuration.md#health-und-uptime-kuma).
Expose Modbus and monitoring only on your trusted network; neither provides authentication.

## Configuration

YAML, TOML and JSON are supported. See [config.example.yaml](config.example.yaml)
and the [configuration reference](docs/configuration.md) for timeouts, multiple
listeners, Unit-ID mapping and RTU over TCP.

```sh
modbus-relay --check -c config.yaml
modbus-relay --help
```

Logs go to stderr. Set `RUST_LOG=info` or `RUST_LOG=modbus_relay=debug`.
Configuration changes require a restart. Only read functions 1–4 are retried by
default; write retries require explicit configuration.

Docker Hub publishing: [first release guide](docs/dockerhub.md).
Maintenance: [automatic updates and releases](docs/updates.md).

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the pinned Rust toolchain and checks.
[Tests](docs/testing.md), [benchmarks](docs/optimization.md) and
[upstream review](docs/cross-project-audit.md) document validation and limitations.

GPL-3.0-only. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
