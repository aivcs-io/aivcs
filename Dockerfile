# Production Dockerfile for AIVCS CLI & Server
# Image: ghcr.io/aivcs-io/aivcs:latest
FROM rust:1.80-slim-bookworm as builder

WORKDIR /usr/src/aivcs
RUN apt-get update && apt-get install -y pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p aivcs-cli --bin aivcs

# Runtime minimal stage
FROM debian:bookworm-slim

WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates git curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/aivcs/target/release/aivcs /usr/local/bin/aivcs

ENTRYPOINT ["aivcs"]
CMD ["--help"]
