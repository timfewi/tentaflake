{ buildGoModule, lib }:

buildGoModule {
  pname = "hermes-auditd";
  version = "0.1.1";
  src = ./.;
  vendorHash = "sha256-+b+JmOmyut/bhwQSsmv49fpRfi4cW1qxrQoDbs9CMBE=";
  subPackages = [
    "cmd/hermes-auditd"
    "cmd/hermes-top"
  ];

  meta = {
    description = "Filesystem audit daemon for Hermes agent state directories";
    longDescription = ''
      Watches Hermes agent state directories for filesystem changes (create, write,
      remove, rename, chmod) using fsnotify. Events are debounced (100ms coalescing
      window per file) and persisted to SQLite with configurable retention.
    '';
    license = lib.licenses.mit;
    homepage = "https://github.com/timfewi/tentaflake";
    maintainers = [ ];
    platforms = lib.platforms.linux;
  };
}
