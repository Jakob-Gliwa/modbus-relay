# syntax=docker/dockerfile:1
FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.1-alpine@sha256:2c27e67049f17e8b67d882a2cf2f5e1a1d4d71c206ec97f91edf278254688e95 AS chef
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
COPY examples ./examples
RUN cargo chef prepare --recipe-path recipe.json

# Exportable dependency layers, independent of source contents and release tag.
# Plain cache mounts would be empty on a fresh GitHub-hosted runner.
FROM chef AS dependencies
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=secret,id=build_ca \
    if [ -f /run/secrets/build_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/build_ca; fi; \
    cargo chef cook --release --locked --recipe-path recipe.json

FROM dependencies AS build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG RELEASE_VERSION
RUN --mount=type=secret,id=build_ca \
    if [ -f /run/secrets/build_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/build_ca; fi; \
    if [ -n "$RELEASE_VERSION" ]; then export MODBUS_RELAY_VERSION="${RELEASE_VERSION#v}"; fi; \
    cargo build --locked --release && cp target/release/modbus-relay /modbus-relay

FROM dependencies AS test-dependencies
RUN --mount=type=secret,id=build_ca \
    if [ -f /run/secrets/build_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/build_ca; fi; \
    cargo chef cook --release --locked --all-targets --recipe-path recipe.json

FROM test-dependencies AS test
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
COPY examples ./examples
COPY config.example.yaml ./
RUN --mount=type=secret,id=build_ca \
    if [ -f /run/secrets/build_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/build_ca; fi; \
    cargo test --release --locked --all-targets

FROM scratch AS runtime
COPY --from=build /modbus-relay /modbus-relay
COPY LICENSE /LICENSE
USER 65532:65532
EXPOSE 5020 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD ["/modbus-relay", "--healthcheck", "127.0.0.1:8080"]
ENTRYPOINT ["/modbus-relay"]
CMD ["-c", "/config/modbus-proxy.yml"]
