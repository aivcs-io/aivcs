# AIVCS Repository Rules

## Exceptions to Sovereign VCS Policy
While the organization enforces a strict "Exit GitHub" and "No Docker Hub" policy for general repositories:
* **The `aivcs` repository itself is the sole exception.**
* **GitHub Sync**: The repository at `https://github.com/aivcs-io/aivcs` is maintained as the public open-source upstream. When releasing a new version, you MUST push the branch and the version tags to the `origin` remote (GitHub).
* **Docker Hub Publishing**: Pushing version tags to `github.com/aivcs-io/aivcs` triggers GitHub Actions (`docker-publish.yml`) that publish the official public image to Docker Hub (`aivcs/aivcs`) and GHCR (`ghcr.io/aivcs-io/aivcs`). This is the only exception where public registry publication is authorized.
