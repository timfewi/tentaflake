{ config, lib, ... }:
let
  cfg = config.tentaflake.management;
  tailscale = config.services.tailscale;
  flags = tailscale.extraSetFlags ++ tailscale.extraUpFlags;
  sshEnabled =
    lib.any (
      flag:
      lib.elem flag [
        "--ssh"
        "--ssh=true"
      ]
    ) tailscale.extraSetFlags
    && !(lib.elem "--" flags)
    && !(lib.any (flag: lib.hasPrefix "--ssh=" flag && flag != "--ssh=true") flags);
  privateConnectivity = cfg.enable && cfg.transport == "tailscale" && tailscale.enable;
in
{
  options.tentaflake.management = {
    enable = lib.mkEnableOption "private operator management" // {
      default = true;
    };
    transport = lib.mkOption {
      type = lib.types.enum [ "tailscale" ];
      default = "tailscale";
      description = ''
        Reviewed private management transport. Tailscale is the currently
        supported backend; additional transports require an implementation
        and acceptance evidence. This does not select Research Internet egress.
      '';
    };
    ssh.policy = lib.mkOption {
      type = lib.types.enum [
        "tailnet-policy"
        "disabled"
      ];
      default = "tailnet-policy";
      description = ''
        Declared SSH authorization mechanism. tailnet-policy uses Tailscale SSH
        and requires an operator-maintained restrictive remote policy. disabled
        is available for development configurations only. The declaration does
        not verify enrollment, effective remote grants or recovery access.
      '';
    };
    capabilities = {
      privateConnectivity = lib.mkOption {
        type = lib.types.bool;
        readOnly = true;
        internal = true;
        description = "Whether the selected private management service is configured.";
      };
      sshAuthorization = lib.mkOption {
        type = lib.types.enum [
          "tailnet-policy"
          "disabled"
        ];
        readOnly = true;
        internal = true;
        description = "Configured SSH authorization mechanism; remote policy remains unverified.";
      };
    };
  };

  config.tentaflake.management.capabilities = {
    inherit privateConnectivity;
    sshAuthorization =
      if privateConnectivity && cfg.ssh.policy == "tailnet-policy" && sshEnabled then
        "tailnet-policy"
      else
        "disabled";
  };
}
