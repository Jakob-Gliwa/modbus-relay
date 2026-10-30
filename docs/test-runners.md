# Test runners

Measured locally on macOS/ARM64, 2026-10-30, with Rust 1.99.1 and Nextest 0.9.133.
The test binaries were compiled first. Three alternating runs per runner used
all 68 tests and 2,048 cases per property test. Both runners used the same
concurrency: first two threads, then all 10 logical CPUs. No concurrent
builds or load tests ran during measurement. Times include Cargo and process startup.

| Threads | Runner | Median | Range |
|---|---|---:|---:|
| 2 | cargo test | 3.35 s | 3.35–3.57 s |
| 2 | Nextest | 8.31 s | 8.31–9.11 s |
| 10 | cargo test | 2.41 s | 2.03–2.60 s |
| 10 | Nextest | 7.28 s | 7.24–7.37 s |

Use **cargo test locally**: it was about 2.5 times faster with two threads
and 3.0 times faster using all CPUs.
Nextest starts a separate process per test; process startup can outweigh parallelism
for a small suite. This is a plausible explanation, not a separately measured cause.
[Nextest execution and benchmark guidance](https://nexte.st/docs/benchmarks/).

CI retains Nextest with `--profile ci`, CPU-based concurrency and no automatic
retries. The local result does not establish which runner wins on GitHub.
After publishing the repository, run **Actions → CI → Run workflow** with
`benchmark_runners` enabled. Download `linux-test-runner-comparison` to compare
both runners on the same Linux VM and with that VM's CPU count.

```sh
python3 scripts/benchmark-test-runners.py
```

Raw results: [two threads](test-runner-results.json),
[all CPUs](test-runner-results-all-cpus.json). Detailed local logs stay under
`target/runner-benchmark` and are not published.

CI uses separate Rust and coverage caches, native AMD64/ARM64 Docker runners and
BuildKit caches per architecture. Superseded branch/PR runs are cancelled. Release
verification calls CI once; release builds reuse its cache, then smoke-test the
uploaded digests before assigning public tags. CI and Docker Hub have not yet been
executed remotely; their timing and cache hit rates still need a first real run.
