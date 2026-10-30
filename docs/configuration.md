# Configuration reference

## Konfiguration und Migration

YAML (`.yaml`, `.yml`), TOML und JSON werden unterstützt. Beispiel:

```yaml
monitoring:
  bind: 0.0.0.0:8080
  health_max_age: 60
  max_connections: 16
devices:
  - name: inverter
    modbus:
      url: inverter.lan:502
      timeout: 2
      attempts: 1
      connection_time: 0.1
      reconnect_delay: 0.1
      request_delay: 0
      idle_time: 0
      connection_ttl: 0
      retry_writes: false
    listen:
      bind: 0:5020
    unit_id_remapping:
      1: 0
```

Das bisherige `devices[].modbus`, `listen.bind`, `timeout`, `connection_time` und
`unit_id_remapping` bleiben erhalten. `tcp://host:port`, `host:port`, `0:port`,
`:port` und `[IPv6]:port` funktionieren; ohne Port gilt 502. Statt `listen` kann
`listeners: [{bind: '0:5020'}, {bind: '0:5021'}]` verwendet werden. Diese Listener
teilen denselben Worker und dasselbe Clientlimit. Dasselbe Backend nicht nochmals
als separates Gerät konfigurieren; identische Adressstrings werden abgewiesen.
DNS-Aliase desselben Geräts müssen vom Betreiber zusammengefasst werden.

| Einstellung | Standard | Bedeutung |
|---|---:|---|
| `modbus.timeout` | 10 s | Getrenntes Limit für Verbindungsaufbau und vollständigen Write/Read-Versuch |
| `modbus.attempts` | 2 | Gesamtzahl Versuche für FC 1–4; 1 minimiert Verzögerung bei Ausfällen |
| `modbus.retry_count` | — | Veralteter Alias: `attempts = retry_count + 1`; nicht zusammen verwenden |
| `modbus.retry_writes` | false | Auch andere Funktionscodes wiederholen; kann Schreibbefehle doppelt ausführen |
| `modbus.connection_time` | 0 s | Wartezeit nach erfolgreichem Verbindungsaufbau |
| `modbus.reconnect_delay` | 0 s | Mindestwartezeit nach Fehler vor dem nächsten Versuch, auch über Anfragen hinweg |
| `modbus.request_delay` | 0 s | Mindestpause ab Ende eines Versuchs bis zum nächsten |
| `modbus.idle_time` | 0 s | Inaktive Backend-Verbindung schließen; 0 deaktiviert |
| `modbus.connection_ttl` | 0 s | Verbindung nach dieser Lebensdauer erneuern; aktive Antwort wird abgewartet |
| `max_clients` | 128 | Gleichzeitige Clients pro Gerät, über alle Listener |
| `queue_capacity` | 128 | Maximal wartende Anfragen pro Gerät |
| `queue_timeout` | 10 s | Maximal zulässiges Alter beim Verlassen der Warteschlange |
| `request_timeout` | automatisch | Gesamtbudget ab vollständiger Anfrage: Queue, Connect, Pausen, alle Versuche |
| `client_idle_timeout` | 300 s | Client ohne neue Daten schließen |
| `frame_timeout` | 5 s | Vollständigen Client-Frame lesen bzw. eine Antwort schreiben |

Zeiten sind Sekunden, auch als Dezimalzahl. Aktive Timeouts müssen positiv und
endlich sein. Unbekannte Felder und ungültige Grenzen werden beim Start abgewiesen.
`--check` prüft ohne Netzwerkzugriff. CLI-Optionen siehe `--help`; `-c/--config-file`,
`-b/--bind`, `--modbus`, `--timeout`, `--modbus-connection-time` sind kompatibel.
`--modbus` zusammen mit `-c` fügt ein Gerät hinzu.

Ohne `request_timeout` wird das Budget aus `queue_timeout + attempts *
(2 * modbus.timeout + connection_time + reconnect_delay + request_delay)`
berechnet. So bleiben auch bewusst langsame Geräte unterstützt. Ein explizites
Budget sollte kleiner als das Antwortzeitlimit des Clients sein, mit Platz für
Netzwerk und Antwortübertragung. Das Lesen des Client-Frames und das Schreiben
der Antwort haben jeweils ein eigenes `frame_timeout`.

Bewusste Unterschiede zur Python-Version:

- Antworten erhalten exakt Transaktions-ID und Unit-ID der jeweiligen Anfrage
  zurück. Mehrere Client-Units dürfen auf dieselbe Backend-Unit zeigen.
- Backend-Transaktions-IDs werden pro Versuch fortlaufend vergeben und überprüft.
  Auch Unit-ID, Function-Code, Protokoll-ID und Framelänge werden validiert.
- Standard-FC 1–6, 15–17, 22 und 23 werden zusätzlich auf PDU-Länge,
  Mengen-/Adressgrenzen und Byteanzahl geprüft. Ungültige Anfragen erhalten
  `0x02` bzw. `0x03`, bevor sie das Gerät erreichen. Schreibbestätigungen müssen
  Adresse, Wert/Maske bzw. Menge korrekt zurückgeben; Leseantworten müssen zur
  angefragten Menge passen. Andere TCP-Funktionscodes bleiben unverändert.
- Bei endgültigem Gerätefehler erhält der Client Gateway-Exception `0x0B`.
  Volle oder abgelaufene Queues liefern `0x06` (busy). Abgelaufene Queue-Einträge
  werden nicht gesendet. Ihre Antwort kann erst nach dem aktuell laufenden Versuch
  kommen; `queue_timeout` ist kein Gesamtzeitlimit für eine Transaktion.
  Das zusätzliche Gesamtbudget begrenzt auch diese Wartezeit. Bei dessen Ablauf
  erhält der Client `0x0B`; abgelaufene Aufträge werden nicht mehr gestartet.
  Ein bereits gesendeter Schreibbefehl kann trotzdem ausgeführt worden sein.
- Nur FC 1–4 werden standardmäßig wiederholt. Unbekannte Codes und Schreibzugriffe
  erhalten einen Versuch. Auch ein fehlgeschlagener Schreibversuch kann vom Gerät
  bereits ausgeführt worden sein; der Proxy kann das nicht rückgängig machen.
- `logging.root.level` wird aus vorhandener Python-Konfiguration übernommen.
  Python-spezifische Handler, Formatter, Dateirotation und externe `.ini`-Logdateien
  werden nicht ausgeführt; beim Import erscheint eine Warnung. Logs gehen nach
  stderr, Filter über `RUST_LOG=info` oder `RUST_LOG=modbus_relay=debug`.
  Der schnelle `tracing-subscriber::filter::Targets` unterstützt globale Level und
  Modulpräfixe, auch kombiniert (`warn,modbus_relay=debug`). Span-/Feldfilter und
  reguläre Ausdrücke aus `EnvFilter` werden nicht unterstützt.
- IPv6 benötigt eckige Klammern. Unbegrenzte Timeouts werden nicht übernommen.
  Konfiguration wird beim Start geladen; Änderungen benötigen einen Neustart.

## RTU over TCP

Optional `modbus.mode: rtuovertcp`, bei echoenden Gateways zusätzlich
`strip_rtu_echo: true`. Clients sprechen weiterhin Modbus TCP. Unterstützt werden
FC 1–6, 15–17, 22, 23 und deren Exceptions. CRC und optionales Echo werden geprüft.
Unbekannte RTU-Funktionslayouts und Unit 0/Broadcast werden vor dem Senden mit
Illegal-Function beantwortet. Native serielle Ports und ASCII sind nicht enthalten;
beides gehörte auch nicht zum implementierten Upstream-Master.

Alle Upstream-PRs sind in [docs/upstream-audit.md](upstream-audit.md) eingeordnet.
Der [projektübergreifende Audit](cross-project-audit.md) verknüpft zusätzliche
Fehlerkorrekturen mit konkreten Regressionstests. SunSpec-Registerkonvertierung ist
bewusst nicht implementiert. Gültige TCP-PDUs bleiben unverändert, außer den
dokumentierten MBAP-/Unit-ID-Anpassungen; ungültige Standard-PDUs werden abgewiesen.

## Architektur, Ressourcen und Shutdown

Der Datenpfad benötigt keine Registerbibliothek: TCP-PDUs werden in festen
260-Byte-Puffern transportiert, RTU in 256-Byte-Puffern. Eine begrenzte MPSC-Queue
und ein Worker besitzen die einzelne Geräteverbindung. Pro Client wird jeweils
eine Anfrage gleichzeitig verarbeitet. Ein fester Empfangspuffer von zusätzlich
260 Bytes pro TCP-Verbindung liest verfügbare Daten gemeinsam ein und erhält
nachfolgende Frames; weitere Pipeline-Daten bleiben im TCP-Puffer des Kernels.
Es gibt keinen Polling-Timer pro Client und kein Cache für Registerdaten.

Die Standardeinstellung nutzt einen Tokio-Eventloop-Thread. `--workers N` aktiviert
mehrere Threads für viele unabhängige Geräte. Ein einzelnes Gerät bleibt absichtlich
seriell, da viele Geräte nur eine offene Anfrage vertragen. `TCP_NODELAY` ist auf
beiden Seiten aktiv. Mehr Parallelität kann die physische Antwortzeit des Geräts
nicht verkürzen. [Benchmark-Anleitung](testing.md) für Durchsatz, Latenz und RSS.
Die [Optimierungsmessungen](optimization.md) vergleichen Logging, Socketzugriffe,
Antwortkanäle und Compilerprofile mit archivierten Quellen und Rohdaten.

Bei Timeout, Protokollfehler oder Abbruch einer Backend-Transaktion wird deren
Socket verworfen. Eine getrennte Clientverbindung lässt eine bereits gesendete
Transaktion zu Ende lesen, damit die Antwort nicht zum nächsten Client gelangt.
SIGINT/SIGTERM stoppt die Annahme neuer Arbeit, lässt aktive Transaktionen bis zu
5 Sekunden auslaufen und schließt danach alle Tasks/Sockets. Wartende Anfragen
werden beim Shutdown verworfen. Ein interner Worker-/Listenerfehler beendet den
Prozess mit Fehlerstatus; der Container-Restart kann ihn neu starten.
