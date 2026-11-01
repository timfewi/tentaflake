# Serializable projection; adapters own runtime facts and native implementation.
{
  schemaVersion = 1;
  presets = builtins.mapAttrs (_: adapter: adapter.preset) (
    import ../adapters {
      pkgs = null;
      lib = null;
    }
  );
}
