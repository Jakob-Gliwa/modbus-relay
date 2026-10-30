# Build- und CI-Cache

Die vollständige Suite läuft im PR und auf dem tatsächlichen Merge-Commit.
Der Release prüft über die GitHub-API, ob genau dieser Commit die jüngste passende
`main`-CI samt allen sieben Pflichtjobs bestanden hat. Er wiederholt diese Suite
nicht. Die hochgeladenen AMD64-/ARM64-Images erhalten weiterhin jeweils einen
nativen Smoke-Test und eine Prüfung ihrer Versionsausgabe vor dem Tagging.

## Docker

- `cargo-chef` erzeugt ein Rezept aus Manifesten, Lockfile und Target-Struktur.
  Normale Änderungen an Rust-Dateien verändern dieses Rezept nicht.
- Produktions- und Testabhängigkeiten werden vor den echten Quellen gebaut.
  Original-Manifeste werden danach wiederhergestellt; Chef verwendet Platzhalter.
- Die Rust-Version bleibt 1.99.1. Das Chef-Buildimage ist zusätzlich per Digest
  festgelegt. Im finalen Scratch-Image befindet sich kein Buildwerkzeug.
- Test- und Produktionsgraph haben getrennte GHA-Cache-Scopes pro Architektur.
  Jeder Build importiert nur seinen eigenen Graphen erneut. Derselbe Builder
  allein garantiert keine Wiederverwendung importierter Zwischenstufen;
  überlappende Importe können unterschiedliche Elternschichten auswählen.
- Release-Jobs lesen den Produktionscache von `main`. Sie exportieren keinen
  zusätzlichen Tag-Cache, den `main` später ohnehin nicht lesen könnte.
- `RELEASE_VERSION` wirkt erst beim Anwendungsbuild. Die Abhängigkeiten werden
  bei einem neuen Versionstag wiederverwendet; die Binary muss für ihre neue
  Versionsausgabe noch einmal gebaut werden.

Das Produktionsprofil bleibt O3, Fat LTO, ein Codegen-Unit. Auch die vollständigen
optimierten Tests bleiben erhalten. `sccache` wurde nicht ergänzt: Es kann die
Linkschritte für Binaries nicht cachen. Ein einfacher BuildKit-Cache-Mount wird
auf frischen GitHub-Runnern nicht automatisch aus `type=gha` wiederhergestellt.

## Messen

Unter **Actions → CI → Run workflow** die Option `benchmark_docker_cache` wählen.
Nach den regulären Builds misst jeder native Docker-Runner einen warmen Build und
eine reine Quellcodeänderung in einem temporären Kontext. Dabei dürfen keine
Abhängigkeiten neu kompiliert werden. Die JSON-Ergebnisse werden als Artefakte
`docker-cache-benchmark-amd64` und `docker-cache-benchmark-arm64` gespeichert.
Nur für diese Messung werden die Caches zusätzlich lokal exportiert und bei
jedem Messlauf importiert. Reguläre CI-Läufe benötigen diesen Export nicht.

Lokal nach einem Test- und Image-Build:

```sh
python3 scripts/benchmark-docker-cache.py
```

Bei zuvor importierten Caches die Quellen mit `--test-cache-from` und
`--runtime-cache-from` erneut angeben, jeweils etwa `type=local,src=/tmp/test-cache`.

Das Ergebnis liegt unter `target/docker-cache-benchmark.json`. Der ursprüngliche
Checkout bleibt unverändert. Bei Lockfile-, Compiler- oder Buildimage-Updates muss
der Abhängigkeitscache neu aufgebaut werden; kalte und warme Builds getrennt
vergleichen. Runner-Wartezeit gehört nicht zur gemessenen Buildzeit.

Quellen: [Docker-Cache auf GitHub Actions](https://docs.docker.com/build/ci/github-actions/cache/),
[cargo-chef](https://github.com/LukeMathWalker/cargo-chef),
[sccache-Einschränkungen](https://github.com/mozilla/sccache/blob/main/docs/Rust.md).
