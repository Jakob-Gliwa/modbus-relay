# Automatische Updates und Releases

Dependabot prüft Rust-Abhängigkeiten täglich, Actions und Docker-Basisimages
wöchentlich. GitHubs Sicherheitsupdates reagieren unabhängig davon auf bekannte
Schwachstellen. Die vollständige CI läuft zusätzlich täglich mit `cargo deny`.

Automatisch gemergt werden ausschließlich Dependabot-PRs, die nur `Cargo.lock`
ändern: stabile Patchupdates innerhalb derselben Major-/Minor-Version, ohne neue
oder entfernte Crates und ohne Quellenwechsel. Andere Änderungen bleiben zur
manuellen Prüfung offen, auch wenn sie eine Sicherheitslücke beheben.

`main` verlangt einen PR und alle sieben CI-Checks auf aktuellem Basisstand.
Die Automatisierung umgeht diese Regeln nicht. Ihr Workflow führt ausschließlich
Code von `main` aus und lädt keinen PR-Code mit Schreibrechten.

Nach erfolgreicher CI auf einem neuen `main`-Commit entsteht automatisch die
nächste Patchversion, zum Beispiel `v0.1.1`. Das gilt auch für manuell gemergte
Änderungen. Der Release-Workflow verlangt eine erfolgreiche `main`-CI für exakt
den Commit des Tags, einschließlich aller sieben Pflichtjobs im aktuellen
CI-Versuch. PR-Läufe und ein älterer Erfolg vor einem neueren Fehler reichen nicht.
Er baut AMD64 und ARM64 und testet die hochgeladenen Images. Erst danach setzt er die Docker-Tags der Version
und `latest`. Veröffentlichungen laufen nacheinander.

Die Docker-Binary meldet bei `--version` die Release-Version. Lokale Builds
verwenden weiterhin die Entwicklungsversion aus `Cargo.toml`.

Fehler blockieren Merge beziehungsweise Veröffentlichung. Ein fehlgeschlagener
Release wird nicht stündlich erneut versucht: Ursache beheben und den Lauf unter
**Actions → Publish Docker Hub image → Re-run failed jobs** erneut starten.
Ein bereits veröffentlichter Git-Tag wird nicht verschoben oder gelöscht.
Auch manuelle Release-Tags benötigen erfolgreiche `main`-CI für denselben Commit.
Die vollständige Suite wird im Release nicht nochmals ausgeführt.

**Actions → Automate tested updates → Run workflow** startet die Prüfung manuell.
Zusätzlich läuft sie nach der CI und stündlich, um unterbrochene Abläufe aufzufangen.
Zum Pausieren diesen Workflow deaktivieren. Security-PRs und CI bleiben aktiv.

Laufende Installationen werden nicht automatisch aktualisiert. Für ein Update
den gewünschten Versionstag in `MODBUS_RELAY_IMAGE` setzen und `docker compose
pull` sowie `docker compose up -d --no-build` mit dieser Variable ausführen.
