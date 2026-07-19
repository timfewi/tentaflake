# NixOS VM integration test for the agent-host runtime path.
#
# `nix flake check` only proves the config *evaluates and builds*. This test
# boots a real VM and asserts the things the template actually promises:
#   - the `tentaflake` CLI is present and runs,
#   - the dynamic status banner renders,
#   - the audit daemon comes up and creates its SQLite event DB,
#   - a declared agent produces its systemd unit, system user, and state dir.
#
# Agents are declared with `autoStart = false` so the VM never tries to pull
# a container image over the (sandboxed, offline) network — we assert each
# unit is *defined*, not that the container is running. A second agent from a
# different runtime (OpenCode) proves the multi-runtime discovery path.
#
# Run just this check:
#   nix build .#checks.x86_64-linux.vm-integration -L
{
  self,
  mkHermesAgent,
  mkOpenCodeAgent,
  ...
}:
{
  name = "tentaflake-integration";

  nodes.machine =
    { lib, ... }:
    {
      imports = [
        self.nixosModules.default
        # One declarative Hermes agent, kept stopped so no image pull happens.
        (mkHermesAgent {
          name = "test";
          autoStart = false;
        })
        # One OpenCode agent (second runtime), also stopped — exercises the
        # mkOpenCodeAgent builder and multi-runtime discovery.
        (mkOpenCodeAgent {
          name = "code";
          hostPort = 4096;
          autoStart = false;
        })
        # A second OpenCode agent WITH servePort, so `nix flake check` actually
        # evaluates the credential assertion and the tailscale-serve unit that
        # only exist on that branch. envFile is just a path here — the unit is
        # never started, so nothing reads it and no secret enters the store.
        (mkOpenCodeAgent {
          name = "pub";
          hostPort = 4097;
          servePort = 8443;
          envFile = "/etc/opencode-pub.env";
          autoStart = false;
        })
      ];

      # The serve unit is wantedBy multi-user.target, but this VM has no
      # tailscaled (and no real env file) — keep it defined-but-not-started.
      systemd.services."opencode-pub-tailscale-serve".wantedBy = lib.mkForce [ ];

      # OCI backend + docker are wired in the template's configuration.nix, which
      # we don't import here (it pulls in hardware config / my-agents.nix); set
      # the pieces the agent unit needs directly.
      virtualisation.oci-containers.backend = "docker";
      virtualisation.docker.enable = true;

      tentaflake = {
        hostName = "agent-host";
        adminUser = "admin";
        # The VM test harness owns the bootloader and networking to the tailnet;
        # disable the template's versions so they don't fight the test rig.
        boot.enable = false;
        tailscale.enable = false;
        # The test harness defines nixpkgs.config read-only; leaving this on
        # would redefine allowUnfree and break evaluation.
        nixSettings.enable = false;
        shell.enable = true;
        auditd.enable = true;
      };
    };

  testScript = ''
    machine.wait_for_unit("multi-user.target")
    # docker.service is socket-activated; wait on the socket, then the first
    # `docker` call below triggers the daemon.
    machine.wait_for_unit("docker.socket")

    with subtest("tentaflake CLI is installed and runs"):
        machine.succeed("command -v tentaflake")
        # `help` needs no daemon; `ps` shells out to the docker backend.
        machine.succeed("tentaflake help")
        machine.succeed("tentaflake ps")

    with subtest("status banner renders and names the host"):
        banner = machine.succeed("tentaflake-status")
        assert "agent-host" in banner, f"hostname missing from banner:\n{banner}"

    with subtest("audit daemon is up and opened its event DB"):
        # The daemon opens no socket — it writes events to SQLite and the
        # separate tentaflake-console service is the HTTP surface. Type=simple
        # marks the unit active before the binary runs, so wait for the DB file
        # rather than asserting it right after wait_for_unit.
        machine.wait_for_unit("tentaflake-auditd.service")
        machine.wait_for_file("/var/lib/hermes-audit/events.db")

    with subtest("declared agent produced its systemd unit"):
        # oci-containers names the unit docker-<container>.service.
        machine.succeed("systemctl cat docker-hermes-test.service")

    with subtest("declared agent produced its system user and state dir"):
        machine.succeed("id hermes-test")
        machine.succeed("test -d /var/lib/hermes-test")
        # State dir must be private (0700) per the template's isolation contract.
        perms = machine.succeed("stat -c '%a' /var/lib/hermes-test").strip()
        assert perms == "700", f"expected 0700 state dir, got {perms}"

    with subtest("second-runtime (OpenCode) agent produced its unit and 0700 state dir"):
        # mkOpenCodeAgent runs as an anonymous uid (65534) — no NixOS system
        # user — so we assert the unit, the private state dir, and the workspace.
        machine.succeed("systemctl cat docker-opencode-code.service")
        machine.succeed("test -d /var/lib/opencode-code")
        oc_perms = machine.succeed("stat -c '%a' /var/lib/opencode-code").strip()
        assert oc_perms == "700", f"expected 0700 opencode state dir, got {oc_perms}"
        machine.succeed("test -d /var/lib/opencode-code/workspace")

    with subtest("servePort agent defines a tailscale-serve unit that tears the serve down"):
        # Defined, not started: no tailscaled in this VM. The ExecStop is the
        # point — without it the agent stays published after the unit goes away.
        unit = machine.succeed("systemctl cat opencode-pub-tailscale-serve.service")
        assert "serve --bg --https=8443 127.0.0.1:4097" in unit, unit
        assert "serve --https=8443 off" in unit, unit
  '';
}
