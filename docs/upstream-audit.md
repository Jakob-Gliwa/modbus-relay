# Upstream-Abgleich

Geprüft am 30.10.2026. Basis: `master` / Version 0.8.0,
Commit `93b3350577b3762c1e6b496c67f2d9b547dce9a0` (05.06.2025).
README, Implementierung, bestehende Tests und alle acht offenen PR-Branches wurden
gelesen. Die Rust-Implementierung übernimmt die Verhaltensanforderungen und ergänzt
eigene Tests; Python-Code wird nicht zur Laufzeit eingebettet.

## Bestehende Funktionen

| Upstream-Funktion | Umsetzung / Nachweis |
|---|---|
| Mehrere Clients, serieller Zugriff pro Gerät | `concurrent_clients_reusing_transaction_ids_share_one_backend`, `fifo_order_and_no_overlapping_requests` |
| Mehrere Geräte | `multiple_listeners_share_backend_and_devices_are_independent` |
| Persistente Verbindung, Reconnect, Timeout, Startverzögerung | Retry-/Timeout-/TTL-Integrationstests |
| Transparente Funktionscodes/PDU | `forwards_fragmented_pipeline_and_vendor_pdu`, Property-Test aller ADU-Größen |
| Unit-ID-Mapping | Anfragebezogene Rückübersetzung; `many_to_one_mapping_restores_each_request_unit` |
| YAML/TOML/JSON | `upstream_formats_and_mapping` |
| CLI und Config + CLI | `tests/cli.rs` |
| Docker | Scratch-Image; `scripts/docker-smoke.py` |
| Logging-Konfiguration | `logging.root.level` migriert, `RUST_LOG`; Python-Handler/Formatter sind ausdrücklich keine kompatible Rust-API |

## Offene Pull Requests

| PR / geprüfter Head | Entscheidung | Tests / Begründung |
|---|---|---|
| [#61](https://github.com/tiagocoutinho/modbus-proxy/pull/61), henfri, `210124c` | **Kritisch, umgesetzt und erweitert.** Antwortidentität prüfen; bei Fehler/Timeout Socket verwerfen. Backend-TID unabhängig vom Client vergeben. | `mismatched_tid_unit_function_and_protocol_reset_connection`, `late_response_after_timeout_never_reaches_next_client`, `disconnected_client_reply_is_drained_before_next_exchange` |
| [#62](https://github.com/tiagocoutinho/modbus-proxy/pull/62), sphings79, `1ee1ec0` | **Kritisch, umgesetzt.** Exception-PDU als zwei Bytes lesen, MBAP-Länge korrigieren. Abweichung führt danach zu neuem Socket, statt nur bereits verfügbare Padding-Bytes zu verwerfen. | `exception_lengths_correct_short_padded_and_delayed_padding` prüft gültige, zu kurze, gepolsterte und verzögert gepolsterte Antworten. Framing-Override gilt nur für Antworten. Ungültige MBAP-Grenzen bleiben Fehler. |
| [#60](https://github.com/tiagocoutinho/modbus-proxy/pull/60), henfri, `ff38924` | **Kritisch, umgesetzt.** Konfigurierbare `attempts`, `retry_count`-Migration und `reconnect_delay`. Zusätzlich sichere Standardeinstellung für Schreibzugriffe. | `retry_count_alias_and_conflicts`, `retries_are_configurable_and_writes_are_not_replayed_by_default`, `retries_recover_on_fresh_socket_with_backoff` |
| [#52](https://github.com/tiagocoutinho/modbus-proxy/pull/52), droscy, `ac65730` | **Umgesetzt.** Idle-Abbau, TTL, Request-Pause, Reconnect-Pause; ein Worker verhindert das Schließen mitten in einer Transaktion. | `idle_and_ttl_release_backend_and_reconnect`, `ttl_never_interrupts_active_exchange_and_pacing_is_enforced` |
| [#51](https://github.com/tiagocoutinho/modbus-proxy/pull/51), droscy, `bc3ca3e` | **Umgesetzt.** Gerätename, Peer-Adresse und prozessweit fortlaufende Client-ID in Verbindungslogs; Client-ID, Client-/Backend-TID und Versuch bei Dispatch bzw. Fehlern. | Keine Hash-ID nötig; Namen sind validiert und Anfragen können zur jeweiligen Verbindung zurückverfolgt werden. |
| [#55](https://github.com/tiagocoutinho/modbus-proxy/pull/55), domob1812, `e7c91c9` | **Optionaler RTU-Transport umgesetzt, bewusst enger begrenzt.** Standard-FC-Längen statt CRC als beliebiger Frame-Begrenzer; CRC, Echo und Unit prüfen. | `canonical_crc_vector`, `rtu_does_not_split_on_accidental_crc_prefix`, `rtu_transport_echo_crc_errors_and_unsupported_functions`. Proprietäres FC 0x41 bleibt für TCP möglich, RTU benötigt ein explizites Layout. |
| [#57](https://github.com/tiagocoutinho/modbus-proxy/pull/57), ChristophCaina, `1a707e7` | **Mehrere Listener umgesetzt; SunSpec-Umwandlung zurückgestellt.** Keine kritische Fehlerkorrektur und keine bestehende Master-Funktion. | Der int16/Scale-Factor-/float32-Umbau verändert Registerbreite und Interpretation. Saubere Unterstützung braucht ein eigenes Modell für Adressräume, Teil-Lesezugriffe, ungültige Werte und Schreibsemantik; eine transparente Proxy-Implementierung soll keine impliziten Datenumwandlungen vornehmen. |
| [#46](https://github.com/tiagocoutinho/modbus-proxy/pull/46), tiagocoutinho, `683bfb7` | **Architektur berücksichtigt.** Der Python-1.0-Umbau wird durch den Rust-Worker und seine begrenzte FIFO ersetzt. | Keine zusätzliche Protokollfunktion; vorhandenes Verhalten wird durch die Kompatibilitäts- und Netzwerktests abgedeckt. |

## Zusätzliche Härtung

- Maximal 260 Bytes pro TCP-ADU, 256 pro RTU-ADU; keine Allocation nach untrusted
  MBAP-Längen. Fuzz-artige Property-Tests für beliebige Bytefolgen.
- Begrenzte Clients/Queues und HTTP-Verbindungen; Deadline für vollständige Frames.
- Kein unkontrolliertes Retry eines Schreibbefehls; Timeout-/Abbruchpfade verwerfen
  Streams. Keine Hintergrund-Abfragen für Health.
- Bind aller Listener vor dem Start; ein fehlgeschlagener Bind hinterlässt keinen
  teilweise gestarteten Dienst. Deterministischer, zeitlich begrenzter Shutdown.

## Geschlossene PRs und ergänzende Issues

Der Folgeabgleich erfasst **alle 32 PRs (8 offen, 24 geschlossen)**. Das
[Inventar mit Status und Head-SHA](pr-inventory.json) macht den Stand nachvollziehbar.
Die geschlossenen PRs wurden nach Laufzeitrelevanz eingeordnet; relevante Codepfade
wurden mit der Rust-Implementierung und ihren Regressionstests verglichen.

| Geschlossene PRs | Bewertung / Nachweis |
|---|---|
| [#17](https://github.com/tiagocoutinho/modbus-proxy/pull/17) | Kritisch: Backend-Fehler darf nicht den ganzen Listener schließen. Getrennte Lebensdauer; `repeated_disconnects_and_backend_failures_release_resources`, Geräteisolation und Reconnect-Tests. |
| [#26](https://github.com/tiagocoutinho/modbus-proxy/pull/26) | Unit-Mapping übernommen und um eindeutige Rückübersetzung pro Anfrage erweitert; `many_to_one_mapping_restores_each_request_unit`. |
| [#5](https://github.com/tiagocoutinho/modbus-proxy/pull/5), [#6](https://github.com/tiagocoutinho/modbus-proxy/pull/6) | Verbindungs-Startpause und mehrere Bridges vorhanden; Warmup-Deadline, mehrere Geräte/Listener sowie Config+CLI getestet. |
| [#7](https://github.com/tiagocoutinho/modbus-proxy/pull/7), [#15](https://github.com/tiagocoutinho/modbus-proxy/pull/15), [#37](https://github.com/tiagocoutinho/modbus-proxy/pull/37) | Docker-Entrypoint/Argumente, Config-Pfad und Ablehnung fehlender Konfiguration berücksichtigt; CLI- und finales Container-Smoke-Test. Entfernte Python-Logdateioption wird nicht wieder eingeführt. |
| [#8](https://github.com/tiagocoutinho/modbus-proxy/pull/8), [#10](https://github.com/tiagocoutinho/modbus-proxy/pull/10), [#11](https://github.com/tiagocoutinho/modbus-proxy/pull/11) | Tests, testbare Lifecycle-API, Konfiguration, Cleanup/Formatierung; eigene Rust-Integrationstests, Port 0 für Tests und klare Shutdown-Verantwortung. Der in #8 eingeführte und in #17 korrigierte gekoppelte Close-Pfad wird nicht übernommen. |
| [#9](https://github.com/tiagocoutinho/modbus-proxy/pull/9), [#12](https://github.com/tiagocoutinho/modbus-proxy/pull/12), [#13](https://github.com/tiagocoutinho/modbus-proxy/pull/13), [#20](https://github.com/tiagocoutinho/modbus-proxy/pull/20), [#36](https://github.com/tiagocoutinho/modbus-proxy/pull/36), [#38](https://github.com/tiagocoutinho/modbus-proxy/pull/38), [#39](https://github.com/tiagocoutinho/modbus-proxy/pull/39) | Python-Packaging, PyPI, tox/PYTHONPATH, Dependency-/Runtime-Updates. Keine zusätzlich zu portierende Protokollfunktion; Cargo.lock, CI, Linux-Teststage und Multiarch-Build erfüllen die entsprechenden Rust-/Docker-Anforderungen. |
| [#21](https://github.com/tiagocoutinho/modbus-proxy/pull/21), [#40](https://github.com/tiagocoutinho/modbus-proxy/pull/40), [#42](https://github.com/tiagocoutinho/modbus-proxy/pull/42), [#43](https://github.com/tiagocoutinho/modbus-proxy/pull/43), [#44](https://github.com/tiagocoutinho/modbus-proxy/pull/44), [#49](https://github.com/tiagocoutinho/modbus-proxy/pull/49) | Config-, systemd-, Kubernetes- und Installationsdokumentation. Docker Compose und Beispielkonfiguration sind vorhanden; zusätzliche systemd-/Kubernetes-Pakete sind nicht nötig für die gewünschte Docker-Bereitstellung. |
| [#56](https://github.com/tiagocoutinho/modbus-proxy/pull/56) | Geschlossener Vorläufer von #57. Mehrere Listener übernommen; SunSpec-Konvertierung weiterhin ausdrücklich ausgenommen. |

Issues zu Socket-Lecks, Mehrclient-Fehlern, Timeouts/Freezes, SolarEdge-Pausen,
Hostnamen, privilegierten Ports und ARM64 wurden ergänzend geprüft. Nachweise,
verbleibende Einschränkungen und neue Korrekturen aus fünf Vergleichsprojekten
stehen im [projektübergreifenden Audit](cross-project-audit.md).
