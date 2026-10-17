# Home-manager module for Striper speech-to-text
#
# Provides a systemd user service for autostart.
# Usage: imports = [ striper.homeManagerModules.default ];
#        services.striper.enable = true;
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.striper;
in
{
  options.services.striper = {
    enable = lib.mkEnableOption "Striper speech-to-text user service";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "striper.packages.\${system}.striper";
      description = "The Striper package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.striper = {
      Unit = {
        Description = "Striper speech-to-text";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${cfg.package}/bin/striper";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
