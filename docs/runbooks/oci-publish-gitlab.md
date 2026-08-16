# OCI publish — GitLab CI (sovereign path)

Publish the **aivcs CLI** OCI image to GAR without GitHub Actions. Builds use
**Nix** (`flake.nix` → `.#aivcs-cli-image`); push uses **Rust** `aivcs oci publish`
(skopeo under the hood).

Part of [lornu-ai/plans#71](aivcs://lornu-ai/plans/issues/71) (sunset GHA).

## Image

```
us-central1-docker.pkg.dev/gcp-lornu-ai/lornu/aivcs:0.3.2
```

Tags on `develop` push: `0.3.2`, `sha-<short>`, `develop`, `latest`.

## Local dry-run (build only)

```bash
cargo run -p aivcs-cli -- oci publish --target aivcs-cli --dry-run
```

## GitLab CI

Pipeline: [`.gitlab-ci.yml`](../../.gitlab-ci.yml)

Required CI/CD variable (masked):

| Variable | Description |
|----------|-------------|
| `GCP_ACCESS_TOKEN` | Short-lived OAuth token for GAR (`oauth2accesstoken`) |

Mint via WIF in-cluster or operator `gcloud auth print-access-token` for one-off
pushes — never commit tokens.

Override tags:

```bash
export AIVCS_OCI_TAGS="0.3.2,canary"
cargo run -p aivcs-cli -- oci publish --target aivcs-cli
```

## Manual publish (operator)

```bash
export GCP_ACCESS_TOKEN="$(gcloud auth print-access-token)"
export CI_COMMIT_SHA="$(git rev-parse HEAD)"
export CI_COMMIT_REF_NAME="$(git branch --show-current)"

cargo run -p aivcs-cli -- oci publish --target aivcs-cli --system x86_64-linux
```

## Consumers

- `lornu-ai/infra-code#221` — Cloudflare LB audit CronJob on `lornu-gke-prod`
- [Sovereign infra runbook](./sovereign-infra-gitlab.md)

## Related

- [plans#71](aivcs://lornu-ai/plans/issues/71)
- [stevedores-org/aivcs#281](aivcs://stevedores-org/aivcs/issues/281)
