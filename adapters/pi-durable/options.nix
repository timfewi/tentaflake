{ lib, ... }:
{
  options.tentaflake.piDurableRuntime = {
    enable = lib.mkEnableOption "the source-qualified Pi Durable runner with an explicitly admitted immutable image bundle";
    transport = lib.mkOption {
      type = lib.types.nullOr (lib.types.enum [ "runsc-pi-durable-v1" ]);
      default = null;
      description = "Explicit opt-in to the reviewed rootful gVisor host Unix transport. Configuration is not runtime acceptance.";
    };
    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = "Reviewed tentaflake-pi-durable package from the exact pinned SDK source; never an instance definition.";
    };
    bundle = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = "Parent-built immutable bundle containing image.tar.gz and image-id, tied to package.";
    };
    image = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Exact immutable image selector recorded by the reviewed bundle; no mutable caller override.";
    };
  };
}
