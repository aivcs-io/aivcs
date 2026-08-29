# AIVCS (AI Version Control System)

The **aivcs** engine (Rust CLI `aivcs` + `aivcsd` daemon): state commits, Content-Addressable Storage (CAS), semantic merge, and Forge v2 orchestration for AI agent swarms and sovereign repositories. Open source at [github.com/aivcs-io/aivcs](https://github.com/aivcs-io/aivcs).

---

## Quick Installation

### 1. Homebrew (Recommended on macOS/Linux)
```bash
brew install aivcs-io/tap/aivcs
```

### 2. Prebuilt Binaries (GitHub Releases)
```bash
# macOS (Apple Silicon)
curl -sL https://github.com/aivcs-io/aivcs/releases/download/v0.4.4/aivcs-darwin-arm64 -o ~/.local/bin/aivcs
chmod +x ~/.local/bin/aivcs

# Linux (x86_64)
curl -sL https://github.com/aivcs-io/aivcs/releases/download/v0.4.4/aivcs-linux-x86_64 -o ~/.local/bin/aivcs
chmod +x ~/.local/bin/aivcs
```

### 3. Docker Container
```bash
docker run --rm ghcr.io/aivcs-io/aivcs:0.4.4 --help
```

---

## Authentication & Login

### Laptop Login (Browser PKCE)

Default on a machine with a display: authorization-code + PKCE against the issuer
(loopback callback). Falls back to device flow if PKCE fails.

```bash
export AIVCS_ISSUER_URL=https://issuer.aivcs.io
aivcs login --issuer https://issuer.aivcs.io
```

### Headless / Device Flow (CI & Non-Display Environments)

```bash
aivcs login --device --issuer https://issuer.aivcs.io
```

Open the printed **Verification URL**, sign in if asked, then **type** the user
code shown in the terminal. Do not rely on a pre-filled complete link — typing
binds the terminal session to the browser subject.

### Check Session
```bash
aivcs login status
```

`login status` shows issuer, scopes, TTL, and (when present) account/roles claims.
### Other Environments
| Situation | Command |
| :--- | :--- |
| **On cluster / with kubecreds** | `aivcs login --in-cluster` |
| **Via Tailscale** | `aivcs login --tailscale` *(or `--url https://...ts.net`)* |
| **Custom Forge** | `aivcs login --url https://forge-v2.aivcs.io --issuer https://issuer.aivcs.io` |

---

## Core Workflows

### 1. Clone a Sovereign Repository
```bash
aivcs clone aivcs://aivcs/infra-code-micro ./infra-code-micro
```

### 2. Publish Changes to Forge v2
```bash
aivcs publish --repo aivcs/my-repo --remote https://forge-v2.aivcs.io -m "Initial commit"
```

### 3. Snapshots & Semantic Merging
```bash
# Snapshot agent state
aivcs snapshot --state state.json --message "Exploration milestone"

# View state log
aivcs log

# Branch and merge
aivcs branch create experiment-1
aivcs merge experiment-1 --target main
```

---

## Documentation

- **[Getting Started Guide](docs/getting-started.md)**
- **[Architecture & CAS Design](docs/architecture.md)**
- **[Runbooks](docs/runbooks/)**
