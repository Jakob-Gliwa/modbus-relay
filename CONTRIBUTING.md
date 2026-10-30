# Development

Install rustup; the repository selects Rust 1.99.1, including rustfmt, Clippy,
LLVM coverage tools and rust-src. Use rust-analyzer in your editor.
Docker and CI use the same compiler version. Cargo.lock is committed.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo nextest run --locked --all-targets
cargo nextest run --release --locked --all-targets
cargo test --locked --doc
cargo llvm-cov --locked --all-targets --ignore-filename-regex 'examples/' --fail-under-lines 95
cargo deny check
```

Install development commands with `cargo install --locked cargo-nextest
cargo-llvm-cov cargo-deny`. They are development tools, not runtime dependencies.
Run `cargo test --locked --all-targets` if nextest is unavailable.
Tests use loopback sockets and simulated devices; no real hardware is needed.
Set `PROPTEST_CASES=2048` for extended property checks.

Before a release, build and smoke-test both Docker architectures. See
[testing.md](docs/testing.md). Historical benchmark results use Rust 1.95;
re-measure before making performance claims about newer compilers.

Never commit local device configurations, credentials, certificates, environment
files, build output or raw captures. Review staged files and run a redacted
secret scan before pushing. Dependency updates must pass the same checks.

Maintainer publishing: this checkout enables `.githooks/pre-push`, which checks
the destination, authenticated GitHub account and commit history with Gitleaks. It permits only
`Jakob-Gliwa/modbus-relay` under `Jakob-Gliwa`. Install Gitleaks before pushing. The guard contains no credentials.

When upgrading Rust, update rust-toolchain.toml, Cargo.toml, Dockerfile and CI
together, then rerun debug/release tests, coverage and Docker smoke tests.
