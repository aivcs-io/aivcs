# aivcs

The **aivcs** binary — the AI Version Control System engine (Rust CLI `aivcs` + `aivcsd` daemon): state commits, semantic merge, and CI runs for agent and source workflows. Open source at [github.com/aivcs-io](https://github.com/aivcs-io).

Built as an OCI image via [oci-builds](aivcs://aivcs/oci-builds) and deployed as a service via [infra-code](aivcs://aivcs/infra-code).

> This repo is the **engine binary** only. The aivcs.io VCS **management tool** (UI + API) lives in [web-apps](aivcs://aivcs/web-apps) and [apps-middle-ware](aivcs://aivcs/apps-middle-ware) and consumes this engine.

## Documentation

All documentation is centralized in [code-governance](aivcs://aivcs/code-governance):

- **[aivcs docs](aivcs://aivcs/code-governance/tree/main/docs/source-repos/aivcs)** — architecture, CLI surface, build (oci-builds), and deployment (infra-code).

See [code-governance](aivcs://aivcs/code-governance) for all organization documentation.
