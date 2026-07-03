{ buildGoModule, lib }:

buildGoModule {
  pname = "hermes-auditd";
  version = "0.1.2";
  src = ./.;
  vendorHash = "sha256-+b+JmOmyut/bhwQSsmv49fpRfi4cW1qxrQoDbs9CMBE=";
  subPackages = [
    "cmd/hermes-auditd"
    "cmd/hermes-top"
    "cmd/tentaflake-console"
  ];

  meta = {
    description = "Filesystem audit daemon + Agent Console for Hermes agent state directories";
    longDescription = ''
      Watches Hermes agent state directories for filesystem changes (create, write,
      remove, rename, chmod) using fsnotify. Events are debounced (100ms coalescing
      window per file) and persisted to SQLite with configurable retention.

      Ships three binaries: hermes-auditd (the watcher daemon), hermes-top (a TUI
      activity monitor), and tentaflake-console (a read-only web UI combining a file
      explorer over the agent state dirs with a live activity monitor).
    '';
    license = lib.licenses.mit;
    homepage = "https://github.com/timfewi/tentaflake";
    maintainers = [ ];
    platforms = lib.platforms.linux;
  };
}
