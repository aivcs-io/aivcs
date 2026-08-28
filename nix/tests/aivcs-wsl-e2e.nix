# E2E smoke test: aivcsd starts and the aivcs CLI can init + snapshot.
{ pkgs, aivcsPackage, aivcsdPackage }:

pkgs.testers.nixosTest {
  name = "aivcs-wsl-e2e";

  nodes.machine = {
    config,
    pkgs,
    lib,
    ...
  }: {
    imports = [ ../modules/aivcsd.nix ];

    services.aivcsd = {
      enable = true;
      package = aivcsdPackage;
      settings = {
        GITHUB_TOKEN = "dummy-token-for-e2e";
        CI_WEBHOOK_SECRET = "dummy-secret-for-e2e";
        SURREALDB_URL = "ws://localhost:8000";
        AIVCS_CAS_DIR = "/var/lib/aivcsd/cas";
      };
    };

    services.surrealdb = {
      enable = true;
      dbPath = "memory";
      extraFlags = [ "--user" "root" "--pass" "root" ];
    };

    environment.systemPackages = [ aivcsPackage ];

    system.stateVersion = "25.05";
  };

  testScript = ''
    machine.wait_for_unit("aivcsd.service")
    machine.wait_for_open_port(8080)

    # Verify aivcsd health and version endpoints
    machine.succeed("curl -f http://localhost:8080/health | grep healthy")
    machine.succeed("curl -f http://localhost:8080/version | grep aivcsd")

    machine.succeed("mkdir -p /tmp/aivcs-demo")
    machine.succeed("bash -lc 'cd /tmp/aivcs-demo && aivcs init'")
    machine.succeed("echo '{\"step\":1}' > /tmp/aivcs-demo/state.json")
    machine.succeed("bash -lc 'cd /tmp/aivcs-demo && aivcs snapshot --state state.json --message e2e'")
    machine.succeed("bash -lc 'cd /tmp/aivcs-demo && aivcs log | grep -q e2e'")
  '';
}
