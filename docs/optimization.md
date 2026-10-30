# Gemessene Optimierungen, 30.10.2026

Übernommen sind `tracing-subscriber::filter::Targets` und ein fester Empfangspuffer
pro TCP-Verbindung. Die Wiederverwendung des Antwortkanals wurde implementiert,
getestet und nach der Messung verworfen. Argumente, Hilfe, Farben und weitere
Clap-Features der CLI bleiben erhalten.

## Messaufbau und Grenzen

Alle hier verglichenen Binaries wurden mit Rust **1.95.0**, LLVM **22.1.2** und dem
mit Rust gelieferten `rust-lld` für `aarch64-unknown-linux-musl` gebaut. Sie liefen
nativ auf ARM64 in derselben Docker-Desktop-Linux-VM auf einem Apple M4
(Linux `6.10.14-linuxkit`). Der Host ist macOS 26.7.1. Der Container hat kein externes
Netzwerk; Lastgenerator, Gerät und Proxy kommunizieren über Loopback.

Der Proxy läuft als eigener Prozess mit einem Tokio-Thread, aktivem Monitoring
und genau einer persistenten Geräteverbindung. Jede Antwort und TID wird geprüft.
Nach 100 Warmup-Anfragen starten die Clients gemeinsam. RSS wird alle 25 ms aus
`/proc/<pid>/status` gelesen und umfasst nur den Proxy. Tabellen nennen den Median
der Durchsatzwerte, den Median der p99-Latenzen einzelner Läufe und den Median des
jeweils höchsten abgetasteten RSS. Das ist kein über alle Anfragen gepooltes p99.

Varianten laufen nacheinander in rotierender Reihenfolge; während der Messungen
laufen keine eigenen Builds, Tests oder weiteren Benchmarks. Alle Messwerte,
einschließlich Ausreißern, bleiben in den JSON-Dateien erhalten. Das Hostsystem
und die VM sind nicht vollständig isoliert: kleine Prozentunterschiede sind kein
Beweis für eine universell schnellere Implementierung. Netzwerksystemaufrufe und
Gerätelatenz dominieren diesen Proxy; Mikrobenchmarks allein entscheiden deshalb
nicht über die Auswahl. Frühere macOS-Messungen sind keine direkte Vergleichsbasis.

## Logging

Verglichen wurden der bisherige `tracing-subscriber::EnvFilter`, dessen einfacher
`Targets`-Filter und `ftlog 0.2.18`, zusätzlich mit einem 16-KiB-Ausgabepuffer.
Bei ftlog ist die Queue auf 1.024 Einträge begrenzt und **blockierend**, damit volle
Queues keine Logs verlieren. Beim Beenden wird der Logger vollständig geleert.
Die experimentellen ftlog-Adapter prüfen globale Log-Level; sie sind keine
vollständigen Ersatzimplementierungen für die bisherige Filterkonfiguration.

Der Mikrobenchmark erzeugt pro Lauf eine Million gleichartige strukturierte
Debug-Ereignisse. Ein Writer zählt tatsächlich ausgegebene Zeilen. Bei `info`
müssen es null sein, bei `debug` exakt eine Million. Gemessen wird sowohl die
Produzentenzeit als auch die Zeit einschließlich Entleerung des Loggers.
Vier Läufe je Variante und Level:

| Logger/Filter | Aktives Debug, ns/Ereignis einschließlich Ausgabe |
|---|---:|
| EnvFilter | 338,77 |
| Targets | 338,40 |
| ftlog | 1.976,45 |
| ftlog + 16-KiB-Puffer | 1.927,84 |

Rohdaten des Mikrobenchmarks (local benchmark archive).
Die deaktivierten Aufrufe liegen im synthetischen Loop bei allen Varianten unter
einer Nanosekunde; daraus lässt sich keine relevante End-to-End-Überlegenheit
ableiten. Der sehr schnelle Zähl-Writer ist kein Modell für eine langsame Festplatte.

Zusätzlich lief der vollständige Proxy viermal je Variante mit 32 Clients,
32.000 Anfragen und `RUST_LOG=debug`, stderr in eine Datei. Nach SIGTERM wurden
jeweils exakt **32.100 Dispatch-Einträge** einschließlich Warmup nachgewiesen.
Die angegebene Durchsatzzeit umfasst die Anfragephase; die abschließende Prüfung
nach dem Shutdown zählt nicht dazu.

| Logger/Filter | Anfragen/s | p99, µs | RSS, KiB |
|---|---:|---:|---:|
| EnvFilter | 24.731 | 1.360,5 | 1.824 |
| Targets | 24.914 | 1.357 | 1.580 |
| ftlog | 23.429 | 1.442,5 | 1.664 |
| ftlog + 16-KiB-Puffer | 23.538 | 1.435,5 | 1.736 |

Rohdaten mit aktiven Proxy-Logs (local benchmark archive).
Targets ist hier etwa 5,8 % schneller als gepuffertes ftlog. Gegenüber EnvFilter
liegt der Durchsatz nahe beieinander, Targets benötigt jedoch weniger Speicher
und produziert ein deutlich kleineres Binary. Deshalb bleibt das vorhandene
strukturierte Tracing erhalten, mit **Targets statt EnvFilter**. Die Benchmarks
belegen die Auswahl unter diesen Kandidaten und dieser Last, keine Rangliste
aller Logging-Bibliotheken oder langsamer Log-Ziele.

`RUST_LOG=info`, `off`, Modulpräfixe und Kombinationen wie
`warn,modbus_relay::proxy=debug` bleiben möglich. Span-/Feldfilter und reguläre
Ausdrücke von EnvFilter entfallen. Es gibt keinen zusätzlichen Logging-Thread
oder eine Logging-Queue im Produktionsbuild. Format und stderr-Ausgabe bleiben.

## Socketzugriffe und Antwortkanal

Sechs rotierende Läufe je Variante mit 32 Clients und jeweils 64.000 Anfragen,
`RUST_LOG=info`, ohne künstliche Geräteverzögerung. Alle verwenden zunächst
`opt-level=3`, Thin-LTO und eine Codegen-Unit.

| Änderung gegenüber Ausgangsversion | Anfragen/s | p99, µs | RSS, KiB |
|---|---:|---:|---:|
| Ausgangsversion | 27.037 | 1.302,5 | 1.824 |
| Nur Targets | 27.068 | 1.281,5 | 1.580 |
| Nur ftlog | 26.915 | 1.295,5 | 1.664 |
| Nur Empfangspuffer | 27.562 | 1.252,5 | 1.736 |
| Nur wiederverwendeter Antwortkanal | 26.969 | 1.310,5 | 1.824 |
| Empfangspuffer + wiederverwendeter Antwortkanal | 27.457 | 1.265 | 1.824 |

Rohdaten aller Einzeländerungen (local benchmark archive).
Die Ausgangsversion enthält auch einen langsameren Lauf mit 22.668 Anfragen/s;
er wurde nicht aussortiert. Der Median bleibt bei 27.037.

Der **Empfangspuffer wird übernommen**: rund 1,9 % mehr Durchsatz in dieser Serie.
Der separate `peek` vor Client-Anfragen entfällt. Ein Lesezugriff nimmt bis zu
260 verfügbare Bytes auf; vollständige Frames werden daraus entnommen, überzählige
Bytes erhalten und nur bei Bedarf verschoben. Header und PDU müssen dadurch bei
bereits vollständig verfügbaren Daten nicht getrennt eingelesen werden. Auch die
TCP-Geräteantwort verwendet den Puffer. RTU-Framing bleibt unverändert.

Der Puffer hat eine feste Obergrenze und keine eigene Heap-Allokation. Er erhöht
den Platzbedarf im Zustand einer TCP-Verbindung um 260 Bytes plus Indizes;
gemessener Gesamt-RSS kann wegen Codegröße und Allokator-Größenklassen dennoch
sinken. Tests mit einem zählenden Reader belegen die eingesparten Leseaufrufe,
einschließlich zweier gemeinsam gelesener Frames. Das ist keine Betriebssystem-
Syscall-Aufzeichnung. Bestehende Timeout-/Pipelining-/Reconnect-Tests sowie der
Vergleich gegen den alten Parser sichern die Protokollsemantik ab.

Der experimentelle **Antwortkanal wird verworfen**: ein pro Client wiederverwendeter
Tokio-`watch`-Kanal, mit Generationen gegen verspätete Antworten und Drop-Signalisierung
für verworfene Jobs, brachte keinen messbaren Vorteil. In Kombination mit dem
Empfangspuffer war der Durchsatz geringfügig niedriger und der RSS 88 KiB höher.
Der Produktionspfad bleibt beim einfachen `oneshot` pro Anfrage. Quellen und
Regressionstests des Versuchs sind für spätere Vergleiche archiviert.

## Compilerprofile

Der ausgewählte Quellstand (Targets + Empfangspuffer + oneshot) wurde mit sechs
Profilen gebaut. `codegen-units=1`, `strip=true` und `panic=abort` sind konstant;
es wird kein `target-cpu=native` verwendet, damit das Image portabel bleibt.
Je Profil sechs Läufe mit 32 Clients und 64.000 Anfragen:

| opt-level / LTO | Binary, Bytes | Anfragen/s | p99, µs | RSS, KiB |
|---|---:|---:|---:|---:|
| 3 / thin | 1.701.064 | 27.318 | 1.260,5 | 1.608 |
| 3 / fat | 1.607.672 | 27.372 | 1.271,5 | 1.624 |
| 2 / thin | 1.606.472 | 27.261 | 1.268,5 | 1.528 |
| 2 / fat | 1.525.840 | 27.522 | 1.291 | 1.532 |
| s / fat | 1.343.480 | 27.210 | 1.284 | 1.328 |
| z / fat | 1.246.144 | 26.785 | 1.294,5 | 1.156 |

Rohdaten aller Profile (local benchmark archive).
Binärgrößen stammen vom gleichen Cross-Linker und Target. Buildzeiten in den
Metadaten enthalten unterschiedlich warme Dependency-Caches und dienen deshalb
nicht als fairer Vergleich der Kompiliergeschwindigkeit.

Als Release-Vorgabe bleibt **opt-level=3**, LTO wechselt auf **fat**.
Gegenüber Thin-LTO ist das Binary 5,5 % kleiner. Die Durchsatzunterschiede der
O2-/O3-Profile sind klein und ihre Messbereiche überlappen. O2/fat gewann die erste
kurze Serie knapp; in den längeren Läufen sowie bei einem und 128 Clients lag
O3/fat vorne. Daraus folgt kein statistisch abgesicherter universeller Profilsieger.
O3/fat bewahrt den Schwerpunkt auf Laufzeit und verkleinert das bisherige Profil;
`z` würde nochmals rund 22,5 % Binarygröße sparen, zeigte aber weniger Durchsatz.

## Kontrollmessungen der ausgewählten Kombination

Vier rotierende Läufe pro Kandidat (Ausgangsversion, O3/thin, O3/fat, O2/fat):
1 Client × 10.000 Anfragen, 32 × 5.000 sowie 128 × 500. Die folgende Tabelle
vergleicht jeweils die Ausgangsversion mit **Targets + Empfangspuffer + O3/fat**.

| Clients | Vorher, Anfragen/s | Nachher, Anfragen/s | Änderung | p99 vorher/nachher, µs | RSS vorher/nachher, KiB |
|---|---:|---:|---:|---:|---:|
| 1 | 17.629 | 17.887 | +1.47 % | 73 / 72 | 1.696 / 1.496 |
| 32 | 26.876 | 27.481 | +2.25 % | 1308.5 / 1264 | 1.824 / 1.624 |
| 128 | 26.905 | 27.377 | +1.75 % | 5366 / 5125 | 2.080 / 1.880 |

Rohdaten einschließlich der anderen Profile: 1 Client (local benchmark archive),
32 Clients (local benchmark archive),
128 Clients (local benchmark archive).
Bei 32 Clients sind es etwa **2,3 % mehr Durchsatz und 11,0 % weniger RSS**.
Das Binary sinkt bei gleichem Cross-Linker von **2.477.304 auf 1.607.672 Bytes**
(**35,1 % kleiner**); das ist kein Vergleich mit einem früher anders gelinkten Image.

Bei `--device-delay-us 1000` und acht Clients (vier Läufe à 1.600 Anfragen)
dominieren Gerät und Timer: vorher **265,8**, nachher **263,8 Anfragen/s**;
p99 **32,441 / 32,472 ms**, RSS **1.696 / 1.496 KiB**. Hier gibt es keinen
Geschwindigkeitsgewinn; die Differenz beträgt −0,72 %.
Tokios Timer und die VM liefern keine präzise Mikrosekundenverzögerung: Der
Parameter ist eine angeforderte Schlafdauer, keine garantierte 1-ms-Gerätelatenz.
Rohdaten mit Geräteverzögerung (local benchmark archive).

## Reproduction

The historical measurements above used Rust 1.95.0. The current development
compiler is pinned in rust-toolchain.toml; rerun measurements after compiler updates.
Raw local snapshots and machine metadata are intentionally excluded from releases.

```sh
cargo build --release --locked --bin modbus-relay --example benchmark
target/release/examples/benchmark --clients 32 --requests-per-client 5000
```

Compare variants sequentially on the same machine. The benchmark validates replies
and samples only the proxy process's RSS. See scripts/benchmark-matrix.py for
repeated comparisons. Experiments must use the same compiler and workload.

## Finale Docker-Images

Das lokale Tag **`modbus-relay:local`** enthält beide Plattformen. Beide finalen
Scratch-Images bestanden den Smoke-Test mit Non-Root, read-only, ohne Capabilities,
32 MiB RAM-Limit, 100 TCP-Anfragen, Health, Metrics, eingebautem Healthcheck und
SIGTERM mit Exitcode 0. ARM64 lief nativ, AMD64 emuliert.

| Architektur | Statisches Binary | Docker Content Size, gerundet | Cgroup-Speicher im Smoke-Test |
|---|---:|---:|---:|
| linux/arm64 | 1.607.672 Bytes | 863 kB | 724 KiB |
| linux/amd64 | 1.900.944 Bytes | 905 kB | 3,391 MiB, emuliert |

Docker Content Size bezeichnet hier die komprimierten Inhalte pro Plattform;
`docker image ls --tree` meldet zusätzlich 5,31 MB lokale Disk Usage für beide
Architekturen zusammen einschließlich entpackter Daten. Diese Summe ist nicht die
Downloadgröße eines einzelnen Images. Cgroup-Speicher ist eine Momentaufnahme
und unterscheidet sich vom Prozess-RSS der Benchmarks.

Die Docker-Registry-Metadatenauflösung des Rust-Basisimages blockierte in dieser
Umgebung. Deshalb wurden die statischen Binaries lokal mit Rust 1.95.0 und musl
gebaut und mit den gleichen Scratch-Runtime-Anweisungen verpackt. Die Linux-Suite
lief separat gegen das gewählte Release-Profil; der aktualisierte
`docker build --target test` wurde hier nicht nochmals vollständig ausgeführt.
Der normale Dockerfile baut weiterhin aus dem Quellcode. Die lokale Ersatzroute
ändert weder Runtime-Dateien noch Nutzer, Ports, Healthcheck oder Entrypoint.

Validierung und Image-Hashes (local benchmark archive),
Messumgebung (local benchmark archive).

## Quellen

- [Targets und unterstützte Filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/targets/struct.Targets.html)
- [ftlog-Dokumentation](https://docs.rs/ftlog/0.2.18/ftlog/) und [Quellcode](https://github.com/nonconvex-tech/ftlog)
- [Cargo-Releaseprofile](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Rust-Codegen-Optionen](https://doc.rust-lang.org/rustc/codegen-options/index.html)
