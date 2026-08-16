FROM debian:bookworm-slim

WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates git curl && rm -rf /var/lib/apt/lists/*

COPY target/release/aivcs /usr/local/bin/aivcs

ENTRYPOINT ["aivcs"]
CMD ["--help"]
