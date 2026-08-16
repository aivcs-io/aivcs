{ config, lib, pkgs, ... }:

let
  cfg = config.services.aivcsd;
in
{
  options.services.aivcsd = {
    enable = lib.mkEnableOption "AIVCS daemon (aivcsd)";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "aivcsd from the aivcs flake";
      description = "The aivcsd package to run.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "aivcsd";
      description = "Dynamic user name for the service.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "aivcsd";
      description = "Dynamic group name for the service.";
    };

    stateDir = lib.mkOption {
      type = lib.types.str;
      default = "aivcsd";
      description = "State directory name under /var/lib (systemd StateDirectory).";
    };

    settings = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "Extra environment variables for the daemon process.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.aivcsd = {
      description = "AIVCS daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      serviceConfig = {
        Type = "simple";
        DynamicUser = true;
        StateDirectory = cfg.stateDir;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateTmp = true;
        Restart = "on-failure";
        ExecStart = lib.getExe cfg.package;
      };

      environment = cfg.settings;
    };
  };
}
