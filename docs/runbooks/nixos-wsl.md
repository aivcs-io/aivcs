# NixOS-WSL Runbook

Run AIVCS inside a NixOS-WSL distribution with reproducible tooling and a preconfigured `aivcsd` service.

## Prerequisites

- Windows 11 with WSL2 enabled
- Nix with flakes (`nix flake` works on your host builder)
- Optional: [Attic](https://github.com/zhaofengli/attic) credentials for binary cache

## Build the WSL tarball

From a Linux machine or WSL distro with Nix:

```bash
git clone https://github.com/aivcs-io/aivcs.git
cd aivcs

# Validates the NixOS-WSL configuration
nix build .#nixosConfigurations.aivcs-wsl.config.system.build.toplevel

# Produces the tarball builder (run as root to emit nixos.wsl)
nix build .#nixosConfigurations.aivcs-wsl.config.system.build.tarballBuilder
sudo ./result/bin/nixos-wsl-tarball-builder
```

The builder writes `nixos.wsl` in the current directory.

CI also uploads the tarball builder artifact from the CI pipeline on pushes to `main`.

## Import into WSL

```powershell
wsl --import aivcs-nixos C:\WSL\aivcs-nixos .\nixos.wsl
wsl -d aivcs-nixos
```

## First run inside the distro

```bash
# Validate platform + cache guidance
aivcs env info

# aivcsd is enabled by default via services.aivcsd
sudo systemctl status aivcsd

# Smoke test
mkdir ~/demo && cd ~/demo
aivcs init
echo '{"step":1}' > state.json
aivcs snapshot --state state.json --message "hello from NixOS-WSL"
aivcs log
```

## Environment validation

`aivcs env info` reports:

- Detected platform (`nixos-wsl` when running inside the tarball)
- Nix / Attic availability
- WSL-specific recommendations (keep `.aivcs/` on the Linux filesystem, not `/mnt/c`)

Run `nix develop` in the repo checkout for a hermetic Rust shell on any Linux host.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| CAS persist errors on `/mnt/c/...` | Move the repo and `.aivcs/` under `$HOME` (ext4/vhdx), not DrvFS |
| Slow rebuilds | Export `ATTIC_SERVER` / `ATTIC_CACHE` and authenticate with Attic |
| `aivcsd` fails to start | Check `journalctl -u aivcsd`; the daemon is currently a stub that should exit 0 |
| Flake check fails on macOS | NixOS-WSL checks run only on `x86_64-linux`; use CI or a Linux builder |

## Related docs

- [Local Development](local-development.md)
- [CI Troubleshooting](ci-troubleshooting.md)
