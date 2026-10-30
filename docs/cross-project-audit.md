# Projektübergreifender PR- und Fehlerabgleich

Stand: **30.10.2026**. Ergebnis: Drei Lücken der ersten Rust-Fassung geschlossen
(Gesamtdeadline, Prüfung von Standard-PDUs, Listener während Shutdown), bestehende
Schutzmechanismen mit zusätzlichen Regressionstests abgesichert. Keine zusätzlichen
Produktionsabhängigkeiten, Register-Caches oder aktiven Geräte-Healthchecks.

## Umfang und Nachvollziehbarkeit

Über die öffentliche GitHub-API wurden offene und geschlossene PRs erfasst:

| Repository | Gesichtet | Vertiefung |
|---|---:|---|
| [tiagocoutinho/modbus-proxy](https://github.com/tiagocoutinho/modbus-proxy/pulls?q=is%3Apr) | alle 32 PRs: 8 offen, 24 geschlossen | Alle offenen Branches; Laufzeitänderungen geschlossener PRs; README, Tests und relevante Issues. Einzelentscheidungen im [Upstream-Audit](upstream-audit.md). |
| [tma/mbproxy](https://github.com/tma/mbproxy/pulls?q=is%3Apr) | alle 12 PRs | Deadlines/Retry, Health, Vendor-Funktionen, Cache-Konsistenz |
| [3cky/mbusd](https://github.com/3cky/mbusd/pulls?q=is%3Apr) | alle 30 PRs | TCP-Framing, RTU-Echo/FC23, Exceptions, Verbindungsabbau; ergänzend Issues |
| [slowtec/tokio-modbus](https://github.com/slowtec/tokio-modbus/pulls?q=is%3Apr) | jüngste 100 PRs | Speicherwachstum, Mengenüberlauf, Shutdown, CRC, Panics, TIDs |
| [stephane/libmodbus](https://github.com/stephane/libmodbus/pulls?q=is%3Apr) | jüngste 100 PRs | FC15/FC22, MBAP-Längen, Zeitlimits, Bounds, Unit-IDs |
| [evcc-io/evcc](https://github.com/evcc-io/evcc/pulls?q=is%3Apr+modbusproxy) | 24 Treffer der Suche `is:pr modbusproxy` | Proxy-Zeitlimits, Protokoll-/Adresskonfiguration; zusätzlich Issue #32687 |

Das [PR-Inventar](pr-inventory.json) enthält Nummer, Titel, URL, Status und, soweit
von der jeweiligen API geliefert, Merge-Datum und Head-SHA. Es unterscheidet die
Vollinventare von begrenzten Seiten und Suchergebnissen. „Gesichtet“ bedeutet
Relevanzprüfung; nicht jeder Packaging-/Dokumentations-PR wurde als Code-Audit
behandelt. Bei relevanten Änderungen wurden Beschreibung und gezielt Diffs bzw.
Tests gelesen, insbesondere für die unten neu behobenen Fehler. Das ist kein
vollständiger Audit der gesamten Historie aller Vergleichsprojekte.

Offene PRs sind Hinweise auf mögliche Fehler, keine Bestätigung, dass die jeweilige
Upstream-Korrektur vollständig oder richtig ist. Die Übertragung folgt dem eigenen
Datenpfad und eigenständigen Tests. Mengen, Layouts und PDU-Grenzen wurden zusätzlich
gegen die [Modbus Application Protocol Specification V1.1b3, Abschnitte 4 und 6](https://www.modbus.org/file/secure/modbusprotocolspecification.pdf)
geprüft. Kein fremder Patch wurde als neue Laufzeitbibliothek eingebunden.

## Neu korrigiert

### Ein Gesamtbudget pro Anfrage

[mbproxy #10](https://github.com/tma/mbproxy/pull/10) behandelt Anfragen, die trotz
abgelaufener Wartezeit weiter in Queue, Connect, Pausen oder Retries verbleiben.
Unsere bisher getrennten Zeitlimits begrenzten nicht die gesamte Lebensdauer.

`devices[].request_timeout` gilt jetzt ab vollständig empfangener, validierter
Anfrage über Queue, Verbindungsaufbau, Startpause, Retry-Pausen und alle Versuche.
Client und Worker teilen dieselbe absolute Deadline. Vor einem weiteren Versuch
und vor dem Schreiben wird sie erneut geprüft. Bei Ablauf folgt `0x0B`; der
Worker verwirft einen möglicherweise teilweise verwendeten Socket. Nach einem
abgebrochenen Austausch gelten Request- und Reconnect-Pausen weiterhin. Es gibt
keine verspätete Ausführung eines noch nicht gesendeten Schreibauftrags.

Ohne explizites Limit wird ein konservatives Budget aus den bestehenden
Zeitlimits berechnet. So werden bewusst langsam konfigurierte Geräte nicht durch
ein zusätzliches, zu kleines Standardlimit abgeschnitten. Das separate Zeitlimit
für das Schreiben der Clientantwort beginnt erst nach der Backend-Bearbeitung.
[evcc Issue #32687](https://github.com/evcc-io/evcc/issues/32687) beschreibt verspätete
Antworten im Umfeld der in [PR #31996](https://github.com/evcc-io/evcc/pull/31996)
vereinheitlichten Timeouts; die dort vermutete konkrete Ursache wird hier nicht
als abschließend bewiesen dargestellt.

Nachweise in `tests/relay.rs`:

- `total_deadline_covers_connection_warmup_without_sending_write`
- `total_deadline_covers_retry_backoff_and_never_reconnects_late`
- `total_deadline_expires_queued_write_during_pacing_without_late_execution`
- `total_deadline_cancels_partial_frame_and_next_request_uses_fresh_socket`
- `slow_backend_gets_a_fresh_downstream_write_deadline`

`tests/config.rs::total_budget_preserves_slow_backend_settings_and_validates_override`
prüft die Ableitung und ungültige Overrides. `request_deadlines_total` zählt einmal
pro abgelaufener Anfrage. Bereits gesendete Writes bleiben bei Timeout mehrdeutig;
es gibt keine Exactly-once-Zusage. Der Proxy kennt auch nicht automatisch das
Timeout eines Clients: das eigene Budget muss dazu passend konfiguriert werden.

### Standard-PDUs und Schreibbestätigungen prüfen

[libmodbus #870](https://github.com/stephane/libmodbus/pull/870) zeigt fehlende
FC22-Echoprüfungen, [#887](https://github.com/stephane/libmodbus/pull/887) übergroße
Byteanzahlen bei FC15. [mbproxy #14](https://github.com/tma/mbproxy/pull/14)
unterscheidet echte Vendor-Exceptions von ungültigen Exception-PDUs. Unsere
ursprüngliche Prüfung von MBAP, TID, Unit und Funktion genügte dafür nicht.

`Frame::validate_request` prüft jetzt FC 1–6, 15–17, 22 und 23 auf exakte Länge,
Mengenobergrenzen, Adressbereich und Byteanzahl. Insbesondere gilt für FC15 exakt
`ceil(quantity / 8)`. Ungültige Standardanfragen erreichen das Gerät nicht und
erhalten `0x02`/`0x03`; `invalid_requests_total` zählt sie. Die Adressaddition nutzt
`u32`, Mengenberechnungen ausreichend breite Typen. Das vermeidet den in
[tokio-modbus #387](https://github.com/slowtec/tokio-modbus/pull/387) adressierten
Überlauf. Längenprüfungen erfolgen vor Indexzugriffen, passend zum Fehlermuster in
[libmodbus #839](https://github.com/stephane/libmodbus/pull/839).

`validate_response` prüft Lese-Byteanzahlen und alle Echo-Felder bei Writes:
Adresse/Wert (FC5/6), Adresse/Menge (FC15/16), Adresse/AND-/OR-Maske (FC22).
Exception-Code 0 wird abgewiesen; unbekannte **nichtnull** Exception-Codes bleiben
erhalten. Andere TCP-Funktionscodes, etwa Huawei FC `0x41`, bleiben innerhalb der
253-Byte-PDU-Grenze transparent und werden standardmäßig nicht wiederholt.

Nachweise:

- `standard_request_quantity_address_and_byte_count_boundaries`
- `standard_responses_validate_counts_and_every_write_echo_field`
- `arbitrary_standard_pdus_never_panic` (Property-Test)
- `invalid_standard_requests_never_reach_backend_and_pipeline_recovers`
- `malformed_confirmations_reset_socket_and_vendor_exceptions_survive`
- `forwards_fragmented_pipeline_and_vendor_pdu`

Das ist eine bewusste Kompatibilitätsverschärfung für **fehlerhafte Standard-PDUs**.
Andere Funktionscodes sind keine vollständig semantisch validierten Nachrichten.
Gültige Inhalte werden nicht umgeschrieben oder als Registerdaten interpretiert.

### Listener vor dem Auslaufen aktiver Verbindungen schließen

[tokio-modbus #336](https://github.com/slowtec/tokio-modbus/pull/336) und der frühere
[Ansatz #334](https://github.com/slowtec/tokio-modbus/pull/334) betreffen beim
Serverende verbleibende Clientverbindungen. Unsere Tasks wurden bereits begrenzt
beendet; der Accept-Socket blieb aber bis zum Ende der Drain-Phase gebunden.
`accept_clients` gibt ihn jetzt unmittelbar beim Stop frei. Aktive Antworten dürfen
weiter auslaufen; nach fünf Sekunden werden verbleibende Tasks abgebrochen.

`shutdown_drains_active_reply_and_releases_sockets_and_gauges` prüft jetzt, dass
neue TCP-Verbindungen **während** einer noch laufenden Backend-Antwort abgewiesen
werden. `shutdown_deadline_cancels_partial_backend_frame_and_pending_queue` prüft
den erzwungenen Abbruch samt Queue-/Client-/Socket-Freigabe. Der virtuelle
Shutdown-Timer und die anschließende reale Kernel-FIN-Prüfung sind zeitlich getrennt,
um einen Testfehler durch automatisches Vorspulen virtueller Zeit zu vermeiden.

## Bereits vorhandene Schutzmaßnahmen, zusätzlich überprüft

| Quelle / Fehlermuster | Umsetzung und konkreter Nachweis |
|---|---|
| [mbproxy #6](https://github.com/tma/mbproxy/pull/6): zusätzliche Health-Verbindung stört Geräte mit nur einem erlaubten Client | Passive Health-/Metrics-Endpunkte. `health_scrapes_do_not_create_extra_backend_connections`: vor Nutzverkehr null Backend-Verbindungen, danach weiterhin genau eine trotz wiederholter Scrapes. |
| [mbproxy #8](https://github.com/tma/mbproxy/pull/8): veraltete/überlappende Cache-Daten nach Writes | Kein Register-Cache. `read_write_read_observes_device_state_without_stale_cache` prüft die echte neue Geräteantwort nach einem Write. Cache-Invalidierungsalgorithmen sind daher nicht nötig. |
| [mbusd #132](https://github.com/3cky/mbusd/pull/132): abgeschnittene lange FC23-Antworten | Funktionsabhängige RTU-Länge und MBAP-Länge für TCP. `maximum_fc23_frames_are_not_truncated_in_tcp_or_rtu`: maximale gültige Anfrage mit 121 Schreibregistern, fragmentierte Antwort mit 125 Leseregistern, beide Transporte. |
| [mbusd #135](https://github.com/3cky/mbusd/pull/135): RS485-Echo verwechselt mit Antwort | Opt-in, exakter Echovergleich, gemeinsames Versuchslimit. `rtu_fc6_echo_requires_separate_ack_and_echo_only_times_out`: FC6 ohne Echo, zwei identische Frames bei Echo, Echo allein ohne ACK. Zusätzlich `rtu_echo_mismatch_is_rejected`. |
| [mbusd #43](https://github.com/3cky/mbusd/pull/43): mehrere TCP-Frames gemeinsam eingelesen und überschrieben | Genau einen Frame verarbeiten; überzählige Bytes im festen 260-Byte-Empfangspuffer erhalten, Rest bleibt im Socket. `buffered_reader_preserves_pipeline_and_reduces_read_calls`, `forwards_fragmented_pipeline_and_vendor_pdu`, `fifo_order_and_no_overlapping_requests`. |
| [mbusd #32](https://github.com/3cky/mbusd/pull/32), [libmodbus #881](https://github.com/stephane/libmodbus/pull/881): falsche MBAP-Längen und Desynchronisation | Feste maximale ADU-Größe, exaktes Framing; fehlerhafte Backend-Streams verwerfen. `oversized_zero_short_and_non_modbus_headers_rejected`, `exception_lengths_correct_short_padded_and_delayed_padding`. |
| [mbusd #29](https://github.com/3cky/mbusd/pull/29): RTU-Exceptions verlieren | Explizite Exception-Länge und CRC. `rtu_fixed_variable_exception_crc_and_oversize`; TCP-Vendor-Exceptions zusätzlich im neuen Integrationstest. |
| [libmodbus #882](https://github.com/stephane/libmodbus/pull/882): Receive-Timeout beginnt mit jedem Byte neu | Ein Limit für den ganzen Frame/Austausch. `trickled_bytes_do_not_extend_whole_frame_deadlines` prüft langsame einzelne Bytes auf beiden Seiten. |
| [tokio-modbus #302](https://github.com/slowtec/tokio-modbus/pull/302), [#304](https://github.com/slowtec/tokio-modbus/pull/304): wachsende Buffer durch falsches `reserve` | Feste Arrays (TCP 260, RTU 256 Bytes), begrenzte Queues/Clients; keine aus MBAP-Längen abgeleitete Heap-Allocation. Framing-Property-Tests und `bounded_fifo_queue_rejects_overload_and_expires_without_sending`. |
| [tokio-modbus #347](https://github.com/slowtec/tokio-modbus/pull/347): CRC hängt von Host-Endianness ab | Explizites Little-Endian für CRC, Big-Endian für MBAP/PDU-Wörter. `canonical_crc_vector`. Kein Testlauf auf Big-Endian-Hardware erfolgt. |
| [tokio-modbus #254](https://github.com/slowtec/tokio-modbus/pull/254), [#261](https://github.com/slowtec/tokio-modbus/pull/261): Panic bei falscher Funktion/Disconnect | I/O-Fehlerpfad und Socket-Reset. `mismatched_tid_unit_function_and_protocol_reset_connection`, `repeated_disconnects_and_backend_failures_release_resources`. |
| [libmodbus #871](https://github.com/stephane/libmodbus/pull/871), [tokio-modbus #359](https://github.com/slowtec/tokio-modbus/pull/359): falsche Unit/TID | Identität pro Anfrage; bei Abweichung schließen statt unbeschränkt nach passender Antwort suchen. Identitäts-, Mapping- und Late-Response-Tests. |
| [libmodbus #873](https://github.com/stephane/libmodbus/pull/873): Überlauf beim Exception-FC | Bitweises OR mit `0x80`; ungültige Client-FC 0/128–255 werden abgewiesen. `response_identity_and_exception`, `malformed_clients_are_closed_without_touching_backend`. |
| [libmodbus #862](https://github.com/stephane/libmodbus/pull/862): Recovery-Flush verwirft neue Anfragen | Kein Flush des Clientstreams; defekten Backend-Socket schließen und Framing neu beginnen. Pipeline-/Reconnect-Tests. |
| [mbusd #83](https://github.com/3cky/mbusd/pull/83), [Upstream #17](https://github.com/tiagocoutinho/modbus-proxy/pull/17), [Issue #23](https://github.com/tiagocoutinho/modbus-proxy/issues/23): letzter Client, Gerätestörung oder Ressourcenleck beendet Dienst | RAII und getrennte Listener-/Backend-Lebensdauer. `repeated_disconnects_and_backend_failures_release_resources`: 100 Fehlerzyklen, Freigabe der Slots/Gauges und erfolgreicher anschließender Zugriff; unabhängige Geräte separat getestet. |
| [mbusd Issue #88](https://github.com/3cky/mbusd/issues/88): nicht antwortendes Gerät blockiert weiter | Endliche Attempts, Gesamtdeadline, begrenzte Queue. Timeout-/Deadline-Tests; `stalled_device_does_not_block_a_different_device_or_monitoring`. |
| [evcc #33745](https://github.com/evcc-io/evcc/pull/33745), [#15912](https://github.com/evcc-io/evcc/pull/15912): Port-/Protokollkonfiguration | Explizite Adressnormalisierung inklusive Port 502, IPv6 und Ablehnung unbekannter Schemes; `addresses_support_upstream_syntax_dns_ipv6_and_defaults`. |

## Abgrenzungen und verbleibende Nachweise

- [tokio-modbus #307](https://github.com/slowtec/tokio-modbus/pull/307) erlaubt größere
  Custom-PDUs. Hier bleibt die Standardgrenze bewusst bestehen; kein unbeschränktes
  Vendor-Framing und keine dynamischen großen Puffer.
- [tokio-modbus #361](https://github.com/slowtec/tokio-modbus/pull/361) betrifft das
  Suchen gültiger RTU-Frames in gestörten Streams. Dieser Proxy erwartet geordnetes
  RTU over TCP mit einem ausstehenden Auftrag, keinen nativen Multi-Master-Bus.
  Ungültige CRC/Framing-Daten führen zum Verbindungsabbau.
- Native UART-/RS485-Richtungsschaltung, Baudraten und serielle Device-Reconnects
  aus `mbusd` sind kein Bestandteil unseres TCP-Transports.
- [libmodbus #831](https://github.com/stephane/libmodbus/pull/831) behandelt einen
  C-Speicherfehler bei musl/DNS. Unser Code enthält diesen C-Pfad nicht. Daraus folgt
  keine Zusage zu allen DNS-Problemen: [Upstream Issue #59](https://github.com/tiagocoutinho/modbus-proxy/issues/59)
  meldet standortspezifische Hostnamen-Timeouts ohne hier reproduzierte Ursache.
- [Upstream Issue #54](https://github.com/tiagocoutinho/modbus-proxy/issues/54)
  meldet einen Freeze nach Stunden ohne ausreichenden Reproducer. Begrenzte
  Ressourcen und Zeitlimits adressieren bekannte Ursachen; der kurze Stresstest
  ersetzt keinen mehrtägigen Lauf mit den tatsächlichen Geräten.
- SunSpec-Konvertierung bleibt wie im Upstream-Audit beschrieben ausgenommen.
  Die Vergleichsprojekte erweitern nicht stillschweigend unseren Funktionsumfang.

Laufende Prüfungen und Messwerte stehen in [testing.md](testing.md). Testabdeckung
und dieses Review reduzieren konkrete bekannte Risiken; sie garantieren nicht die
Abwesenheit weiterer Fehler oder vollständige Kompatibilität mit jeder Firmware.
