# aivcs

The public source distribution of the AI Version Control System CLI and runtime.

Canonical documentation and governance live in [`aivcs/code-governance`](https://future.aivcs.io/aivcs/code-governance).

## Install

**Homebrew**

```sh
brew install aivcs-io/tap/aivcs
```

**Container image**

```sh
docker run --rm ghcr.io/aivcs-io/aivcs:latest --help
```

Images are published on tag by the sovereign release pipeline for
`linux/amd64` and `linux/arm64`, with build provenance and an SBOM attached.
Pin a digest rather than `latest` for reproducible use.

**From source**

```sh
cargo install --path crates/aivcs-cli
```

Requires a stable Rust toolchain. `cargo audit` is configured in
`.cargo/audit.toml`; see the comments there for which advisories are suppressed
and why.
