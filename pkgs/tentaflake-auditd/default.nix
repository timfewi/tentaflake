{ buildGoModule, lib }:

buildGoModule {
  pname = "tentaflake-auditd";
  version = "0.2.0";
  src = ./.;
  vendorHash = "sha256-+b+JmOmyut/bhwQSsmv49fpRfi4cW1qxrQoDbs9CMBE=";
  subPackages = [
    "cmd/tentaflake-auditd"
    "cmd/tentaflake-top"
    "cmd/tentaflake-console"
  ];

  # Compat shim for one release: older module generations (and operator habits)
  # still exec hermes-top. Remove together with the other rename shims.
  postInstall = ''
    ln -s tentaflake-top $out/bin/hermes-top
  '';

  meta = {
    description = "Filesystem audit daemon + Agent Console for agent state directories";
    longDescription = ''
      Watches agent state directories (Hermes, ZeroClaw, …) for filesystem
      changes (create, write, remove, rename, chmod) using fsnotify. Events are
      debounced (100ms coalescing window per file) and persisted to SQLite with
      configurable retention.

      Ships three binaries: tentaflake-auditd (the watcher daemon),
      tentaflake-top (a TUI activity monitor), and tentaflake-console (a
      read-only web UI combining a file explorer over the agent state dirs with
      a live activity monitor).
    '';
    license = lib.licenses.mit;
    homepage = "https://github.com/timfewi/tentaflake";
    maintainers = [ ];
    platforms = lib.platforms.linux;
  };
}
