{
  config,
  lib,
  pkgs,
  aivcsPackage,
  aivcsdPackage,
  ...
}:
{
  imports = [ ../modules/aivcsd.nix ];

  wsl.enable = true;
  system.stateVersion = "25.05";

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
  };

  environment.systemPackages = [ aivcsPackage aivcsdPackage ];

  services.aivcsd = {
    enable = true;
    package = aivcsdPackage;
    settings = {
      SURREALDB_ENDPOINT = "memory";
    };
  };

  programs.bash.interactiveShellInit = lib.mkAfter ''
    echo "AIVCS NixOS-WSL — run 'aivcs env info' to validate your environment."
  '';
}
