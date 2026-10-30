# syntax=docker/dockerfile:1
FROM rust:1.99.1-alpine AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=secret,id=build_ca \
    if [ -f /run/secrets/build_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/build_ca; fi; \
    cargo build --locked --release && cp target/release/modbus-relay /modbus-relay

FROM build AS test
COPY tests ./tests
COPY examples ./examples
COPY config.example.yaml ./
RUN --mount=type=secret,id=build_ca \
    if [ -f /run/secrets/build_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/build_ca; fi; \
    cargo test --release --locked --all-targets

FROM scratch
COPY --from=build /modbus-relay /modbus-relay
COPY LICENSE /LICENSE
USER 65532:65532
EXPOSE 5020 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD ["/modbus-relay", "--healthcheck", "127.0.0.1:8080"]
ENTRYPOINT ["/modbus-relay"]
CMD ["-c", "/config/modbus-proxy.yml"]
