{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
in
lib.mkIf cfg.users.enable {
  users.users.${cfg.adminUser} = {
    isNormalUser = true;
    description = cfg.adminDescription;
    extraGroups = [
      "networkmanager"
      "wheel"
    ];
    shell = cfg.adminShell;
    openssh.authorizedKeys.keys = cfg.adminAuthorizedKeys;
  };
}
