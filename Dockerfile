FROM rust:latest AS builder
WORKDIR /src
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release -p aivcs-cli

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates git curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/aivcs /usr/local/bin/aivcs

ENTRYPOINT ["aivcs"]
CMD ["--help"]
