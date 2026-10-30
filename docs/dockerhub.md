# Docker Hub: erster Release

Image: [acidcliff/modbus-relay](https://hub.docker.com/r/acidcliff/modbus-relay).
Version **0.1.0** ist für AMD64 und ARM64 veröffentlicht.
GitHub bleibt unter **Jakob-Gliwa/modbus-relay**; Docker Hub nutzt **acidcliff**.

## Einmalig einrichten

1. Bei Docker Hub als `acidcliff` anmelden und das Repository `modbus-relay`
   im eigenen Namespace anlegen. Für frei verfügbare Images `Public` wählen.
2. Unter den Kontoeinstellungen einen Personal Access Token mit **Read & Write**
   und Ablaufdatum erstellen. Delete-Rechte werden nicht benötigt.
3. Im persönlichen GitHub-Repository unter **Settings → Environments** die
   Umgebung `dockerhub` anlegen. Dort ein Environment Secret namens
   **DOCKERHUB_TOKEN** mit dem Tokenwert speichern.

Den Token direkt bei GitHub hinterlegen, nicht in Dateien, Git oder im Chat.
Der Workflow verwendet `acidcliff` fest als Docker-Login und Image-Namespace.
[Repository anlegen](https://docs.docker.com/docker-hub/repos/create/),
[Docker-Zugangstoken](https://docs.docker.com/security/access-tokens/).

## Release auslösen

Erst den vorbereiteten Quellstand auf GitHub veröffentlichen und eine grüne CI
abwarten. Vor dem Push muss die GitHub CLI als `Jakob-Gliwa` angemeldet sein;
der lokale Push-Hook prüft Account, Ziel und Secrets.

```sh
git tag v0.1.0
git push origin v0.1.0
```

Der Tag startet **Publish Docker Hub image**. Beide Architekturen werden nativ
gebaut. Erst wenn CI und die Smoke-Tests der hochgeladenen Digests bestehen,
werden die gemeinsamen Tags gesetzt: `0.1.0`, ein Commit-Tag und bei stabilen
Versionen `latest`. Ein fehlgeschlagener Lauf kann ungetaggte Digests hinterlassen.
Manuelle Workflow-Starts sind ebenfalls möglich, aber nur auf einem `v*`-Tag.

## Image verwenden

Nach dem erfolgreichen Release und mit ausgefüllter `config.yaml`:

```sh
docker pull acidcliff/modbus-relay:0.1.0
MODBUS_RELAY_IMAGE=acidcliff/modbus-relay:0.1.0 docker compose up -d --no-build
```

Docker wählt AMD64 oder ARM64 automatisch. Für reproduzierbare Deployments einen
Versionstag statt `latest` verwenden. Der bestehende lokale Build bleibt ohne
`MODBUS_RELAY_IMAGE` die Standardeinstellung.
