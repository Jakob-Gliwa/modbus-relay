# Tests und Leistungsmessung

## Suite

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --release --locked --all-targets
PROPTEST_CASES=2048 cargo test --locked --test frame
docker build --target test .
docker build -t modbus-relay:local .
python3 scripts/docker-smoke.py modbus-relay:local
```

Die Suite benötigt kein echtes Modbus-Gerät, Internet oder fest vergebene Ports.
Integrationstests binden ausschließlich Loopback auf Port 0. Die Sandbox mancher
Entwicklungsumgebungen muss lokale Listener erlauben. Alle Abhängigkeiten stammen
aus `Cargo.lock`; sie müssen vor einem Offline-Lauf bereits im Cargo-Cache liegen.

Stand 30.10.2026: Rust 1.99.1 besteht Debug- und Release-Tests auf macOS.
Die bisherigen Linux-Prüfungen wurden mit Rust 1.95 ausgeführt; CI prüft den neuen Stand auf beiden Docker-Architekturen.
Nach PR-Audit und Optimierung: **68 Tests auf macOS/arm64 und Linux/arm64 erfolgreich**, dabei
jeweils 2.048 Fälle pro Property-Test. `fmt` und Clippy mit `-D warnings` bestehen.
Gemessene Zeilenabdeckung: **97,12 %**; vollständige Zahlen im
[Coverage-Bericht](coverage-summary.txt). ARM64- und AMD64-Images wurden gebaut und
haben jeweils den Container-Smoke-Test bestanden; AMD64 lief dabei emuliert.
Das gewählte Release-Profil wurde außerdem unter Linux mit allen 68 Tests geprüft.
Buildroute, aktuelle Imagegrößen und Optimierungsmessungen stehen im
[Optimierungsbericht](optimization.md).

- Framing: Grenzlängen, beschädigte Header, fragmentierte und zusammenhängende
  Nachrichten, Exceptions, unbekannte TCP-Funktionscodes, CRC-Referenzvektor,
  zufällige PDUs und Bytefolgen mit Proptest.
  Gepuffertes Lesen wird zusätzlich gegen den bisherigen Parser verglichen;
  gemeinsame Frames, byteweise Fragmentierung, Pufferkompaktierung, maximale
  Framegrößen, Exception-Reparatur und tatsächliche Leseaufrufe werden geprüft.
- Echte TCP-Verbindungen: 32 konkurrierende Clients mit wiederverwendeten TIDs,
  FIFO, maximal eine Backend-Verbindung, Mapping, getrennte Clients, späte/falsche
  Antworten, Timeouts, Retries, Schreibbefehle, Queue-Überlast und Ablauf.
- PR-Regressionen: absolutes Gesamtbudget über Queue/Connect/Pausen/Retry,
  keine verspäteten Writes, korrekte Schreibbestätigungen und Byteanzahlen,
  Mengen-/Adressgrenzen ohne Überlauf, Vendor-Exceptions, maximale FC23-Frames,
  reine RTU-Echos ohne ACK, passive Healthchecks, 100 Fehler-/Disconnect-Zyklen.
- Lifecycle: Leerlauf, TTL, Mindestpausen, Startup-Rollback, unabhängige Geräte,
  Clientlimits, langsame Teilpakete, Graceful Shutdown und erzwungener Abbruch
  einer nur teilweise empfangenen Backend-Antwort.
- HTTP: Gesundheitsübergänge unbekannt/gesund/veraltet/fehlerhaft, Metrics,
  Methoden, Headergrenzen, HEAD und Verbindungslimit.
  Health prüft passive Antwortfrische: Verbindungsabbruch bei ungültigem Register
  einschließlich Retry, anschließende Erholung, ausbleibendes Polling und gültige
  Modbus-Exceptions. Die 30-Sekunden-Grenze wird mit virtueller Zeit geprüft.
- CLI: kompatible Optionen, Beispiele, ungültige Parameter, tatsächlicher
  Binary-Start mit einem bzw. mehreren Threads, Healthcheck und SIGTERM.
  Logging umfasst globale Level, Modulfilter, ausgeschaltete Logs und ungültige Filter.
- Docker: echtes finales Scratch-Image als Non-Root, read-only, ohne Capabilities,
  32-MiB-Limit; 100 Modbus-Anfragen, Health, Metrics und sauberer Exitcode.

Backend-Mocks werden mit expliziten Benachrichtigungen koordiniert. Endliche
Mocks werden gejoint, damit deren Assertions nicht unbemerkt verloren gehen.
Fehlerpfade haben begrenzte Wartezeiten; der lange Shutdown-Deadline-Test verwendet
Tokios virtuelle Zeit. Timing-Tests prüfen Mindestpausen und Ablaufgrenzen; die übrigen Tests
vergleichen Protokollresultate und Zustände.

Coverage installieren und messen:

```sh
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
cargo llvm-cov --locked --all-targets --ignore-filename-regex 'examples/' --html
```

CI verlangt mindestens **95 % Zeilenabdeckung des Produktionscodes**. Benchmark-
und Testdateien zählen nicht in diese Quote. Der Bericht unterscheidet Regionen,
Funktionen und Zeilen; Zeilenabdeckung ist keine Garantie für vollständige
Zustands-/Branchabdeckung. Der HTML-Bericht liegt unter `target/llvm-cov/html`.

## Reproduzierbarer Benchmark

```sh
cargo build --release --locked --bin modbus-relay --example benchmark
target/release/examples/benchmark --clients 32 --requests-per-client 5000
```

Die Lastquelle startet ein simuliertes TCP-Gerät und den Proxy als **separaten
Prozess**. Nach 100 Warmup-Anfragen warten alle Clients an einer Barriere. Jeder
Client führt 5.000 Anfrage-/Antwortzyklen aus. Antwortinhalt und Transaktions-ID
werden geprüft. Alle 25 ms wird der RSS des Proxy-Prozesses erfasst: unter Linux
aus `/proc/<pid>/status`, unter macOS mit `ps`. Lastgenerator und Gerät zählen nicht
dazu. Metriken im Rust-Dienst bleiben aktiviert. Standardmäßig wird `RUST_LOG=info`
gesetzt und stderr nach `/dev/null` geleitet. `--log-level debug --log-file /tmp/relay.log`
misst aktives Logging und prüft nach sauberem SIGTERM die vollständige Anzahl der
Dispatch-Einträge einschließlich 100 Warmup-Anfragen.

Der [Optimierungsbericht](optimization.md) enthält wiederholte Linux-Vergleiche
für Logging, Empfangspuffer, wiederverwendete Antwortkanäle und sechs Compilerprofile.
Die nachfolgenden macOS-Zahlen bleiben als historische Ausgangsmessungen erhalten.

Vergleich mit dem geprüften Upstream:

```sh
git clone https://github.com/tiagocoutinho/modbus-proxy.git /tmp/modbus-proxy
git -C /tmp/modbus-proxy checkout 93b3350577b3762c1e6b496c67f2d9b547dce9a0
target/release/examples/benchmark --clients 32 --requests-per-client 5000 \
  --python-source /tmp/modbus-proxy/src/modbus_proxy.py
```

Beide Läufe nacheinander auf einem ansonsten ruhigen System ausführen. Mit
`--device-delay-us 1000` kann eine Geräteverzögerung ergänzt werden (Tokio-Timer
haben Millisekundenauflösung). `--workers 2` untersucht mehrere Runtime-Threads.
Für Aussagen über echte Anlagen zusätzlich mit deren Polling-Intervall, Anzahl
Clients, Gerätelatenz und Netzwerk messen. Weder Register-Caching noch künstliche
Antworten im Proxy verfälschen den Vergleich.

## Gemessener Ausgangswert, 30.10.2026

Apple M4, macOS 26.7.1, arm64; Rust 1.95.0 im Release-Profil; Python 3.9.6;
ein Worker, ein simuliertes Gerät ohne zusätzliche Verzögerung, Loopback-TCP,
32 Clients, 160.000 gemessene Anfragen je Lauf:

| Messung | Rust | Python 0.8.0 |
|---|---:|---:|
| Durchsatz | 15.130 Anfragen/s | 5.593 Anfragen/s |
| p50 Antwortzeit | 2,138 ms | 5,267 ms |
| p95 Antwortzeit | 2,223 ms | 7,824 ms |
| p99 Antwortzeit | 2,338 ms | 13,058 ms |
| Maximal abgetasteter Proxy-RSS | 3.040 KiB | 19.248 KiB |
| Backend-Verbindungen | 1 | 1 |

Rohdaten: Rust (local benchmark archive),
Python (local benchmark archive). In diesem einzelnen Lauf rund
2,7-facher Durchsatz und 84 % weniger RSS. Keine belastbare statistische
Verteilung über wiederholte Läufe und kein hardwareunabhängiges Leistungsversprechen.
Der Container-Smoke-Test auf Linux/arm64 meldete nach 100 Anfragen rund 1 MiB via
`docker stats`; Container-Cgroup-Memory und macOS-RSS sind unterschiedliche Messgrößen.

Die Messung deckte eine unnötige Registrierung bereits abgelaufener Timer auf.
Der Datenpfad prüft jetzt zuerst, ob eine konfigurierte Pause noch aussteht.
Bei deaktivierter Request-/Reconnect-Pause wird kein Warte-Timer angelegt.

## Kontrollmessung nach dem PR-Audit

Gleiche Parameter, nach Abschluss der Builds und Tests, mit den zusätzlichen
Standard-PDU-Prüfungen und absoluten Anfrage-Deadlines:

| Messung | Aktualisierter Rust-Build |
|---|---:|
| Durchsatz | 14.911 Anfragen/s |
| p50 / p95 / p99 Antwortzeit | 2,158 / 2,298 / 2,747 ms |
| Maximal abgetasteter Proxy-RSS | 2.960 KiB |
| Anfragen / Backend-Verbindungen | 160.000 / 1 |

Rohdaten (local benchmark archive). Der Durchsatz liegt
in diesem Lauf etwa 1,4 % unter dem Ausgangslauf. Die Einzelmessungen trennen
Messschwankungen nicht vom Aufwand der zusätzlichen Prüfungen; es ist keine
statistisch abgesicherte Regressionsaussage. Die früheren Vergleichswerte bleiben
als Ausgangsmessung erhalten.

Das neu gebaute finale Image bestand den Smoke-Test für Linux/arm64 nativ und
Linux/amd64 emuliert. Der ARM64-Container meldete dabei 732 KiB via `docker stats`,
der emulierte AMD64-Container 3,445 MiB; beide liefen unter dem 32-MiB-Limit.
Diese Werte sind Momentaufnahmen und keine RAM-Obergrenzen unter beliebiger Last.
Quellen, abgeleitete Fehlerfälle und Testzuordnung: [PR-Audit](cross-project-audit.md).
