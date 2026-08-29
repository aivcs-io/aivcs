# Forge access service

Use AIVCS without `kubectl port-forward`.

## Off-cluster tools and operator workstations

The stable access service is `https://aivcsd.aivcs.io`. The CLI selects it by
default:

```bash
aivcs login
aivcs login status
aivcs clone aivcs://aivcs/aivcs
```

When the public edge is unavailable, use Tailscale subnet routes (no
port-forward):

```bash
# Resolves aivcs-forge-pg ClusterIP via kubectl; HTTP over RFC1918 / tailnet
aivcs login --tailscale --context aivcs-core-live

# Or an explicit HTTPS tailnet hostname when the operator exposes forge
aivcs login --url https://aivcs-forge-pg.<tailnet>.ts.net
```

Authentication uses `AIVCS_TOKEN` or `~/.aivcs/token`. If neither exists,
`aivcs login` may retrieve the token from the configured Kubernetes Secret.
The token is never placed in the URL.

## Kubernetes workloads

Workloads stay on internal service discovery:

```bash
aivcs login --in-cluster
```

This selects `http://aivcs-forge-pg.aivcs-forge-pg.svc.cluster.local`. Use
`--tls --port 443` when the Service terminates HTTPS in-cluster:

```bash
aivcs login --in-cluster --tls --port 443
```

Deployed workloads should normally receive `AIVCS_FORGE_URL` and their token through
their manifest and ESO instead of writing a home-directory session.

## Explicit endpoints

Use `--url` only for another approved access-service endpoint:

```bash
aivcs login --url https://aivcsd.aivcs.io
```

Plain HTTP is rejected except for loopback, Kubernetes Service DNS, and
`aivcs login --tailscale` (RFC1918 / Tailscale CGNAT routes). HTTPS is always
permitted, including `https://*.ts.net` tailnet hostnames. The edge and
in-cluster paths expose the same `/api/v1` CAS contract, so
publish/fetch/clone/push/pull do not need transport-specific behavior.
