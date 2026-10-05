# NixOS VM integration test for the agent-host runtime path.
#
# `nix flake check` only proves the config *evaluates and builds*. This test
# boots a real VM and asserts the things the template actually promises:
#   - the `tentaflake` CLI is present and runs,
#   - the dynamic status banner renders,
#   - a declared agent produces its systemd unit, system user, and state dir.
#
# Agents are declared with `autoStart = false` so the VM never tries to pull
# a container image over the (sandboxed, offline) network — we assert each
# unit is *defined*, not that the container is running. A second agent from a
# different runtime (ZeroClaw) proves the multi-runtime discovery path.
#
# Run just this check:
#   nix build .#checks.x86_64-linux.vm-integration -L
{
  self,
  mkHermesAgent,
  mkZeroClawAgent,
  ...
}:
{
  name = "tentaflake-integration";

  nodes = {
    machine =
      { pkgs, ... }:
      let
        brokerPackage = pkgs.callPackage ../pkgs/tentaflake-broker { };
        networkTestImage = pkgs.dockerTools.buildLayeredImage {
          name = "tentaflake-network-test";
          tag = "latest";
          contents = pkgs.buildEnv {
            name = "tentaflake-network-test-root";
            paths = [
              pkgs.bash
              pkgs.busybox
              pkgs.coreutils
              pkgs.curl
            ];
            pathsToLink = [ "/bin" ];
          };
          config = {
            Cmd = [ "/bin/sh" ];
            Env = [ "PATH=/bin" ];
            User = "65534:65534";
          };
        };

      in
      {
        imports = [
          self.nixosModules.default
          # One declarative Hermes agent, kept stopped so no image pull happens.
          (mkHermesAgent {
            name = "test";
            autoStart = false;
          })
          # One ZeroClaw agent (second runtime), also stopped — exercises the
          # mkZeroClawAgent builder and multi-runtime discovery.
          (mkZeroClawAgent {
            name = "assistant";
            autoStart = false;
          })
        ];

        # OCI backend + docker are wired in the template's configuration.nix, which
        # we don't import here (it pulls in hardware config / my-agents.nix); set
        # the pieces the agent unit needs directly.
        virtualisation = {
          oci-containers.backend = "docker";
          docker.enable = true;
          # Only this disposable guest disk is exposed to installer formatting.
          emptyDiskImages = [
            3072
            1024 # Unrelated swap/LVM must survive installer disk cleanup.
          ];
        };

        # The stopped controller does not pull in its broker at boot. Make the
        # fetch broker an explicit fixture service so the reboot subtest can
        # verify its credential, network, and health restoration independently.
        systemd = {
          services = {
            quota-controller-probe = {
              requires = [ "tentaflake-workspace-quota-hermes-test.service" ];
              after = [ "tentaflake-workspace-quota-hermes-test.service" ];
              serviceConfig.ExecStart = "${pkgs.coreutils}/bin/sleep infinity";
            };
            "tentaflake-broker-fetch-zeroclaw-assistant".wantedBy = [ "multi-user.target" ];
            # Exercise the actual credential unit's mount namespace.
            "tentaflake-broker-credentials-zeroclaw-assistant".serviceConfig.ExecStartPost =
              pkgs.writeShellScript "check-credential-write-boundary" ''
                set -eu
                test -w "$RUNTIME_DIRECTORY"
                ${pkgs.gnugrep}/bin/grep -Eq '^CapBnd:[[:space:]]+0+$' /proc/self/status
                for forbidden in /run/tentaflake-broker/outside /run/tentaflake-broker/other-agent/outside /etc/tentaflake-credential-outside; do
                  if ${pkgs.coreutils}/bin/touch "$forbidden" 2>/dev/null; then
                    echo "credential unit escaped its writable directory: $forbidden" >&2
                    exit 1
                  fi
                done
              '';
          };
          tmpfiles.rules = [ "d /run/tentaflake-broker/other-agent 0700 root root -" ];
        };

        environment = {
          etc = {
            "tentaflake/network-test-image".source = networkTestImage;
            "tentaflake/llm-upstream.py".source = ./fixtures/llm-upstream.py;
            "tentaflake/installer-disk-library".source = ../installer/disk.sh;
          };
          systemPackages = [
            pkgs.btrfs-progs
            pkgs.cryptsetup
            pkgs.dosfstools
            pkgs.e2fsprogs # Only for the explicit legacy-image refusal fixture.
            pkgs.gptfdisk
            pkgs.git
            pkgs.lvm2
            pkgs.nixos-install-tools
            pkgs.python3
            pkgs.restic
            brokerPackage
          ];
        };

        tentaflake = {
          hostName = "agent-host";
          adminUser = "admin";
          # The VM test harness owns the bootloader and networking to the tailnet;
          # disable the template's versions so they don't fight the test rig.
          boot.enable = false;
          # This test exercises the headless runtime, not a physical console.
          # kmscon is unreliable on QEMU's synthetic bochs DRM device.
          modernConsole.enable = false;
          # The test harness defines nixpkgs.config read-only; leaving this on
          # would redefine allowUnfree and break evaluation.
          nixSettings.enable = false;
          shell.enable = true;
          broker.agents.zeroclaw-assistant = {
            enable = true;
            subnet = "10.203.30.0/30";
            gateway = "10.203.30.1";
            fetch = {
              enable = true;
              allowedHosts = [
                "169.254.169.254"
                "example.com"
              ];
            };
          };
          backup = {
            enable = true;
            paths = [ "/var/lib/hermes-test" ];
            repositoryFile = "/run/tentaflake-backup/repository";
            passwordFile = "/run/tentaflake-backup/password";
            initialize = true;
            timerConfig = null;
            pruneOpts = [ ];
            checkOpts = [ "--read-data" ];
          };
          worker = {
            image = networkTestImage;
            imageReference = "tentaflake-network-test:latest";
            agents.hermes-test = {
              enable = true;
              workspace = "/var/lib/hermes-test/workspace";
              maxInboxEntries = 4;
              maxQueueEntries = 2;
              maxQueueBytes = 1024 * 1024;
              maxSnapshotBytes = 16 * 1024 * 1024;
              maxSnapshotEntries = 1000;
              maxTimeoutSeconds = 60;
              cpus = "0.5";
              workspaceTmpfsSize = "8m";
              tmpTmpfsSize = "4m";
            };
            agents.zeroclaw-assistant = {
              enable = true;
              workspace = "/var/lib/zeroclaw-assistant/data";
              containerUid = 65534;
              containerGid = 65534;
              maxSnapshotBytes = 16 * 1024 * 1024;
              maxSnapshotEntries = 1000;
              maxTimeoutSeconds = 10;
              cpus = "0.5";
              workspaceTmpfsSize = "8m";
              tmpTmpfsSize = "4m";
            };
          };
          workspaceQuota.agents = {
            hermes-test = {
              enable = true;
              workspace = "/var/lib/hermes-test/workspace";
              sizeMiB = 128;
              state = {
                path = "/var/lib/hermes-test";
                sizeMiB = 128;
              };
            };
            zeroclaw-assistant = {
              enable = true;
              workspace = "/var/lib/zeroclaw-assistant/data";
              sizeMiB = 128;
              ownerUid = 65534;
              ownerGid = 65534;
              state = {
                path = "/var/lib/zeroclaw-assistant";
                sizeMiB = 128;
              };
            };
          };
        };
      };

    attacker =
      { pkgs, ... }:
      {
        environment.systemPackages = [ pkgs.curl ];
        networking.firewall.enable = false;
        system.stateVersion = "26.05";
      };

    podman =
      { pkgs, ... }:
      let
        podmanTestImage = pkgs.dockerTools.buildLayeredImage {
          name = "tentaflake-podman-test";
          tag = "latest";
          contents = pkgs.buildEnv {
            name = "tentaflake-podman-test-root";
            paths = [
              pkgs.bash
              pkgs.busybox
              pkgs.coreutils
              pkgs.curl
            ];
            pathsToLink = [ "/bin" ];
          };
          config = {
            Cmd = [ "/bin/sh" ];
            Env = [ "PATH=/bin" ];
            User = "65534:65534";
          };
        };
      in
      {
        imports = [
          self.nixosModules.default
          (mkZeroClawAgent {
            name = "podman";
            autoStart = false;
          })
        ];

        virtualisation.oci-containers.backend = "podman";
        virtualisation.podman.enable = true;
        environment.etc."tentaflake/podman-test-image".source = podmanTestImage;

        tentaflake = {
          hostName = "podman-host";
          adminUser = "admin";
          boot.enable = false;
          modernConsole.enable = false;
          nixSettings.enable = false;
          shell.enable = true;
          broker.agents.zeroclaw-podman = {
            enable = true;
            networkName = "tf-zeroclaw-podman";
            subnet = "10.203.50.0/30";
            gateway = "10.203.50.1";
            fetch = {
              enable = true;
              allowedHosts = [ "example.com" ];
            };
          };
        };
      };
  };

  testScript = ''
    import shlex
    import json

    # runsc's sandbox helpers share the container PID cgroup. A limit of 16
    # can reject the sandbox bootstrap before the workload starts, so keep
    # the runtime probes bounded while leaving headroom for gVisor itself.
    runtime_probe_pids_limit = 64

    def check_live_doctor(node, backend, image, network, endpoint):
        name = "tf-doctor-fixture"
        apparmor = "--security-opt=apparmor=docker-default " if backend == "docker" else ""
        node.succeed(
            f"{backend} run -d --name {name} --runtime=runsc --network={network} "
            "--read-only --user=65534:65534 --cap-drop=ALL "
            "--security-opt=no-new-privileges " + apparmor +
            "--memory=64m --memory-swap=64m --cpus=0.5 --pids-limit=64 "
            "--ulimit=nofile=128:128 --ulimit=nproc=64:64 "
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=8m "
            "--tmpfs=/run:rw,nosuid,nodev,noexec,size=8m "
            "--tmpfs=/var/tmp:rw,nosuid,nodev,noexec,size=8m "
            f"{image} sleep 120"
        )
        resources = dict(memoryBytes=67108864, memorySwapBytes=67108864,
                         nanoCpus=500000000, pidsLimit=64, nofile=128, nproc=64)
        tmpfs = {path: 8388608 for path in ("/tmp", "/run", "/var/tmp")}
        fields = ["agent", name, "balanced"] + ["true"] * 17
        fields += ["true", "false", "-", "true", endpoint, network,
                   "[]", json.dumps(tmpfs), "true", json.dumps(resources)]
        manifest = "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue\tfalse\t36\n"
        manifest += "\t".join(fields) + "\n"
        config = (f"backend={backend}\nflake_dir=/fixture\nhost_name=fixture\n"
                  "agents_file=/tmp/doctor-agents.tsv\nsecurity_profile=balanced\n"
                  "security_file=/tmp/doctor-security.tsv\n")
        for path, content in (("/tmp/doctor.conf", config),
                              ("/tmp/doctor-security.tsv", manifest),
                              ("/tmp/doctor-agents.tsv", "")):
            node.succeed(f"printf %s {shlex.quote(content)} > {path}")
        command = "TENTAFLAKE_CONFIG=/tmp/doctor.conf tentaflake doctor --security --json"
        status, report = node.execute(command)
        findings = json.loads(report)["findings"]
        live = [item for item in findings if item["id"] in ("TFSEC-033", "TFSEC-034")]
        if backend == "podman":
            # The pinned Podman/runsc combination reports no AppArmor profile
            # and null capability fields. Preserve the real negative evidence;
            # a synthetic complete-schema unit fixture is not runtime proof.
            observed = json.loads(node.succeed(f"podman inspect {name}"))[0]
            assert observed["AppArmorProfile"] == "", observed
            assert observed["EffectiveCaps"] is None, observed
            assert observed["BoundingCaps"] is None, observed
            assert status == 1, report
            assert any(item["id"] == "TFSEC-034" and item["severity"] == "critical"
                       for item in live), report
            node.succeed(f"podman rm --force {name}")
            return
        if status not in (0, 1) or live:
            print(node.succeed(f"{backend} inspect {name}"))
            raise AssertionError(f"valid {backend} fixture did not pass live inspection: {report}")
        node.succeed(f"{backend} update --memory=128m --memory-swap=128m {name}")
        status, report = node.execute(command)
        assert status == 1, report
        assert any(item["id"] == "TFSEC-034" and item["severity"] == "critical"
                   for item in json.loads(report)["findings"]), report
        node.succeed(f"{backend} rm --force {name}")

    def unit_with_start_script(node, service):
        unit = node.succeed(f"systemctl cat {service}")
        exec_start = next(
            line.removeprefix("ExecStart=").split()[0]
            for line in unit.splitlines()
            if line.startswith("ExecStart=")
        )
        return unit + "\n" + node.succeed("cat " + exec_start)

    def wait_for_broker_health(node, endpoint, diagnostic):
        try:
            node.wait_until_succeeds(
                "curl --noproxy '*' --max-time 2 "
                "--fail --silent "
                f"http://{endpoint}/healthz "
                "> /tmp/tentaflake-broker-health.json && "
                "jq -e '.status == \"ready\"' "
                "/tmp/tentaflake-broker-health.json",
                timeout=30,
            )
        except Exception:
            host, port = endpoint.rsplit(":", 1)
            _, output = node.execute(
                "curl --noproxy '*' --max-time 2 "
                "--silent --show-error --include "
                f"http://{endpoint}/healthz; "
                f"ip -4 route get {host}; "
                f"ss -ltnp 'sport = :{port}'; "
                "nft -a list chain inet nixos-fw input; "
                "nft -a list chain inet nixos-fw input-allow; "
                + diagnostic
            )
            print(output)
            raise

    def worker_diagnostics(node, unit):
        _, output = node.execute(
            f"systemctl status --no-pager -l {unit}; "
            f"journalctl --no-pager -n 100 -u {unit}"
        )
        print(output)

    def wait_for_worker_file(node, unit, path):
        try:
            node.wait_until_succeeds(
                f"test -f {path} || "
                f"systemctl is-failed --quiet {unit}",
                timeout=30,
            )
        except Exception:
            worker_diagnostics(node, unit)
            raise
        status, _ = node.execute(f"test -f {path}")
        if status != 0:
            worker_diagnostics(node, unit)
            raise AssertionError(
                f"worker {unit} failed before publishing {path}"
            )

    # The final subtest exercises a real reboot. Start this node explicitly so
    # the test driver does not add QEMU's `-no-reboot` default.
    machine.start(allow_reboot=True)
    machine.wait_for_unit("multi-user.target")
    attacker.wait_for_unit("multi-user.target")
    podman.wait_for_unit("multi-user.target")
    machine.wait_for_unit("docker.service")
    machine.succeed("test -L /var/run")
    machine.succeed("test -S /run/docker.sock")
    machine.succeed("test -S /var/run/docker.sock")
    failed_units = machine.succeed(
        "systemctl --failed --no-legend --plain"
    ).strip()
    assert failed_units == "", f"failed boot units:\n{failed_units}"

    with subtest("tentaflake CLI is installed and runs"):
        machine.succeed("command -v tentaflake")
        # `help` needs no daemon; `ps` shells out to the docker backend.
        machine.succeed("tentaflake help")
        machine.succeed("tentaflake ps")

    with subtest("status banner renders and names the host"):
        banner = machine.succeed("tentaflake-status")
        assert "agent-host" in banner, f"hostname missing from banner:\n{banner}"
        assert "test" in banner, f"Hermes agent missing from banner:\n{banner}"
        assert " assistant " in banner, f"ZeroClaw agent missing from banner:\n{banner}"
        machine.succeed(
            "tentaflake status --json | "
            "jq -e '.host == \"agent-host\" and "
            ".security_profile == \"balanced\" and "
            "([.agents[].name] | index(\"assistant\")) != null'"
        )
        machine.succeed("test -s /etc/tentaflake/cli.conf")
        machine.succeed("test -s /etc/tentaflake/agents.tsv")

    with subtest("host diagnostics accept stopped agents and redact names"):
        for flag in ["help", "--help", "-h"]:
            machine.succeed(f"TENTAFLAKE_CONFIG=/dev/null tentaflake {flag}")
        reports = []
        for command in ["doctor", "health"]:
            text = machine.succeed(f"tentaflake {command} --hide")
            assert "agent-host" not in text, text
            assert " assistant " not in text, text
            assert " test " not in text, text
            assert "agent-1" in text, text
            report = json.loads(machine.succeed(f"tentaflake {command} --json --hide"))
            assert report["host"] == "redacted", report
            assert report["problems"] == 0, report
            assert report["failed_agents"] == [], report
            assert report["unknown_agents"] == [], report
            assert 0 <= report["disk_percent"] < 90, report
            reports.append(report)
        assert reports[0] == reports[1], reports

        machine.succeed(
            "printf 'hermes\\tmissing-fixture\\tmissing-fixture\\t"
            "tentaflake-diagnostics-nonexistent-fixture.service\\t/tmp/missing-fixture\\n' "
            "> /tmp/tentaflake-diagnostics-missing.tsv"
        )
        machine.succeed(
            "sed 's@^agents_file=.*@agents_file=/tmp/tentaflake-diagnostics-missing.tsv@' "
            "/etc/tentaflake/cli.conf > /tmp/tentaflake-diagnostics-missing.conf"
        )
        code, report = machine.execute(
            "TENTAFLAKE_CONFIG=/tmp/tentaflake-diagnostics-missing.conf "
            "tentaflake doctor --json --hide"
        )
        assert code == 1, report
        assert json.loads(report)["unknown_agents"] == ["agent-1"], report

    with subtest("security doctor accepts the fail-closed balanced capsule"):
        report = machine.succeed("tentaflake doctor --security --json")
        machine.succeed(
            "tentaflake doctor --security --json | "
            "jq -e '.profile == \"balanced\" and "
            "([.findings[].id] | index(\"TFSEC-013\")) != null'"
        )
        assert "TFSEC-002" not in report, report
        assert "TFSEC-011" not in report, report

    with subtest("declared agent produced its systemd unit"):
        # oci-containers names the unit docker-<container>.service.
        unit = machine.succeed("systemctl cat docker-hermes-test.service")
        rendered = unit_with_start_script(machine, "docker-hermes-test.service")
        for flag in [
            "--runtime=runsc",
            "--network=none",
            "--cap-drop=ALL",
            "--read-only",
            "--security-opt=no-new-privileges:true",
            "--security-opt=apparmor=docker-default",
            "--memory=2g",
            "--memory-swap=2g",
            "--cpus=2.0",
            "--pids-limit=512",
        ]:
            assert flag in rendered, f"missing {flag} from secure service:\n{rendered}"
        assert "--network=host" not in rendered, rendered
        assert "--privileged" not in rendered, rendered
        assert " -p " not in rendered, rendered
        assert "Restart=on-failure" in unit, unit
        assert "RestartSec=10s" in unit, unit
        assert "StartLimitIntervalSec=0" in unit, unit
        assert "RestartSteps=5" in unit, unit
        assert "RestartMaxDelaySec=1min" in unit, unit

    with subtest("declared agent produced its system user and state dir"):
        machine.succeed("id hermes-test")
        machine.succeed("test -d /var/lib/hermes-test")
        # State dir must be private (0700) per the template's isolation contract.
        perms = machine.succeed("stat -c '%a' /var/lib/hermes-test").strip()
        assert perms == "700", f"expected 0700 state dir, got {perms}"

    with subtest("second-runtime (ZeroClaw) agent produced its unit and 0700 state dir"):
        # mkZeroClawAgent runs as an anonymous uid (65534) — no NixOS system
        # user — so we assert the unit, the private state dir, and the workspace.
        machine.succeed("systemctl cat docker-zeroclaw-assistant.service")
        machine.succeed("test -d /var/lib/zeroclaw-assistant")
        zero_perms = machine.succeed("stat -c '%a' /var/lib/zeroclaw-assistant").strip()
        assert zero_perms == "700", f"expected 0700 zeroclaw state dir, got {zero_perms}"
        machine.succeed("test -d /var/lib/zeroclaw-assistant/data")

    with subtest("installer formats Btrfs root and records the boot filesystems"):
        script = (
            'set -euo pipefail\nINSTALL_LOG=/tmp/installer-filesystems.log\n'
            'dialog() { :; }\ndie() { if [ -f "$INSTALL_LOG" ]; then cat "$INSTALL_LOG" >&2; fi; echo "$1" >&2; exit 1; }\n'
            'source /etc/tentaflake/installer-disk-library\n'
            'prepare_disk "$1" "$INSTALL_LOG"\n'
        )
        machine.succeed("printf %s " + shlex.quote(script) + " > /tmp/installer-filesystems.sh")
        machine.succeed("printf untouched > /tmp/not-a-disk")
        refused = machine.fail("bash /tmp/installer-filesystems.sh /tmp/not-a-disk 2>&1")
        assert "not a block device" in refused
        assert machine.succeed("cat /tmp/not-a-disk") == "untouched"
        machine.succeed(
            "set -eu; sgdisk -o -n 1:0:+64M -t 1:8200 -n 2:0:+128M -t 2:8e00 -n 3:0:0 /dev/vdc; "
            "udevadm settle; mkswap /dev/vdc1; swapon /dev/vdc1; "
            "pvcreate /dev/vdc2; vgcreate unrelated-vg /dev/vdc2; "
            "lvcreate -L 32M -n preserved unrelated-vg; "
            "pvcreate /dev/vdb; vgcreate selected-vg /dev/vdb; "
            "lvcreate -L 32M -n erased selected-vg"
        )
        machine.succeed("swapon --show=NAME --noheadings --raw | grep -Fx /dev/vdc1")
        machine.succeed("bash /tmp/installer-filesystems.sh /dev/vdb")
        machine.succeed("swapon --show=NAME --noheadings --raw | grep -Fx /dev/vdc1")
        machine.succeed("test -b /dev/unrelated-vg/preserved")
        machine.succeed("test ! -e /dev/selected-vg/erased")
        machine.succeed("findmnt --mountpoint /mnt -n -o FSTYPE | grep -Fx btrfs")
        machine.succeed("findmnt --mountpoint /mnt/boot -n -o FSTYPE | grep -Fx vfat")
        machine.succeed("blkid -s LABEL -o value /dev/vdb2 | grep -Fx nixos")
        machine.succeed("nixos-generate-config --root /mnt --show-hardware-config > /tmp/installer-hardware.nix")
        machine.succeed("grep -F 'fsType = \"btrfs\";' /tmp/installer-hardware.nix")
        machine.succeed("grep -F 'fsType = \"vfat\";' /tmp/installer-hardware.nix")
        machine.succeed("echo installer-persistence > /mnt/probe; umount -R /mnt; mount /dev/vdb2 /mnt")
        assert machine.succeed("cat /mnt/probe").strip() == "installer-persistence"
        # Avoid a duplicate nixos label interfering with the later guest reboot.
        machine.succeed("umount /mnt; btrfs filesystem label /dev/vdb2 installer-test-root")

        # A VG spanning the confirmed disk and another disk must be refused
        # before cleanup, leaving even the selected PV metadata unchanged.
        machine.succeed(
            "set -eu; wipefs -a /dev/vdb2; pvcreate /dev/vdb2 /dev/vdc3; "
            "vgcreate shared-vg /dev/vdb2 /dev/vdc3"
        )
        pv_header = machine.succeed("dd if=/dev/vdb2 bs=1M count=1 status=none | sha256sum")
        refused = machine.fail("bash /tmp/installer-filesystems.sh /dev/vdb 2>&1")
        assert "spans another disk" in refused
        assert machine.succeed("dd if=/dev/vdb2 bs=1M count=1 status=none | sha256sum") == pv_header
        machine.succeed("swapon --show=NAME --noheadings --raw | grep -Fx /dev/vdc1")
        machine.succeed("test -b /dev/unrelated-vg/preserved")

        # Non-luks-prefixed target mappings and LVM-on-crypt need graph-based
        # cleanup, while an unrelated encrypted mapper must remain open.
        machine.succeed(
            "set -eu; vgremove -fy shared-vg; pvremove -fy /dev/vdb2 /dev/vdc3; "
            "cryptsetup open --type plain --key-file /dev/zero --keyfile-size 32 --key-size 256 "
            "/dev/vdb2 installer-target-crypt; "
            "cryptsetup open --type plain --key-file /dev/zero --keyfile-size 32 --key-size 256 "
            "/dev/vdc3 unrelated-crypt; "
            "pvcreate /dev/mapper/installer-target-crypt; "
            "vgcreate encrypted-selected-vg /dev/mapper/installer-target-crypt; "
            "lvcreate -L 32M -n erased encrypted-selected-vg"
        )
        machine.succeed("bash /tmp/installer-filesystems.sh /dev/vdb")
        machine.succeed("test ! -e /dev/mapper/installer-target-crypt")
        machine.succeed("test -b /dev/mapper/unrelated-crypt; test -b /dev/unrelated-vg/preserved")
        machine.succeed("swapon --show=NAME --noheadings --raw | grep -Fx /dev/vdc1")
        machine.succeed("umount -R /mnt; btrfs filesystem label /dev/vdb2 installer-test-root")
        machine.succeed("cryptsetup close unrelated-crypt")
        machine.succeed("swapoff /dev/vdc1; vgchange -an unrelated-vg")

    with subtest("persistent state and workspaces have independent fixed-size filesystems"):
        for name, state, workspace, uid in [
            ("hermes-test", "/var/lib/hermes-test", "/var/lib/hermes-test/workspace", 10000),
            ("zeroclaw-assistant", "/var/lib/zeroclaw-assistant", "/var/lib/zeroclaw-assistant/data", 65534),
        ]:
            for kind, path, image in [
                ("state", state, f"state/{name}.img"),
                ("workspace", workspace, f"{name}.img"),
            ]:
                machine.wait_for_unit(f"tentaflake-{kind}-quota-{name}.service")
                machine.succeed(f"findmnt --mountpoint {path} -n -o FSTYPE | grep -Fx btrfs")
                size = int(machine.succeed(f"stat -c %s /var/lib/tentaflake-workspace-volumes/{image}").strip())
                assert size == 128 * 1024 * 1024, (name, kind, size)
                machine.succeed(f"test $(stat -c %u:%g {path}) = {uid}:{uid}")
                machine.fail(f"fallocate -l 160M {path}/over-quota")
                machine.succeed(f"rm -f {path}/over-quota")
        machine.succeed("test -d /var/lib/hermes-test/skills; test -d /var/lib/hermes-test/cron")
        machine.succeed("test -d /var/lib/zeroclaw-assistant/.zeroclaw/data")

    with subtest("stopping private state ownership stops its dependent controller"):
        machine.succeed("systemctl start quota-controller-probe.service")
        machine.wait_for_unit("quota-controller-probe.service")
        machine.succeed("systemctl stop tentaflake-state-quota-hermes-test.service")
        machine.fail("systemctl is-active --quiet quota-controller-probe.service")
        machine.fail("systemctl is-active --quiet tentaflake-workspace-quota-hermes-test.service")
        machine.succeed("systemctl start tentaflake-workspace-quota-hermes-test.service")
        machine.fail("systemctl is-active --quiet quota-controller-probe.service")
        machine.succeed("systemctl start tentaflake-worker-hermes-test.path")

    with subtest("state initialization refuses an agent-owned directory symlink"):
        machine.succeed(
            "install -d -m 0755 /var/lib/quota-unrelated; "
            "printf '%s' preserve > /var/lib/quota-unrelated/marker; "
            "mv /var/lib/hermes-test/skills /var/lib/hermes-test/skills-saved; "
            "ln -s /var/lib/quota-unrelated /var/lib/hermes-test/skills"
        )
        machine.fail("systemctl restart tentaflake-state-quota-hermes-test.service")
        machine.succeed(
            "test $(stat -c %a /var/lib/quota-unrelated) = 755; "
            "test $(cat /var/lib/quota-unrelated/marker) = preserve; "
            "journalctl -u tentaflake-state-quota-hermes-test.service --no-pager "
            "| grep -F 'refusing symlink or non-directory quota source'"
        )
        machine.fail("systemctl is-active --quiet tentaflake-workspace-quota-hermes-test.service")
        machine.succeed(
            "rm /var/lib/hermes-test/skills; "
            "mv /var/lib/hermes-test/skills-saved /var/lib/hermes-test/skills; "
            "systemctl reset-failed tentaflake-state-quota-hermes-test.service; "
            "systemctl start tentaflake-workspace-quota-hermes-test.service; "
            "systemctl start tentaflake-worker-hermes-test.path"
        )

    with subtest("git remote policy rejects a repository remote changed by the agent"):
        repository = "/var/lib/hermes-test/workspace/remote-fixture"
        machine.succeed(f"git init -q {repository}")
        machine.succeed(
            f"git -C {repository} remote add origin "
            "https://github.com.evil.test/example/allowed.git"
        )
        machine.fail(
            "tentaflake remote-check "
            f"\"$(git -C {repository} remote get-url origin)\" "
            "https://github.com/example/allowed.git"
        )
        machine.succeed(
            f"git -C {repository} remote set-url origin "
            "https://github.com/example/allowed.git"
        )
        machine.succeed(
            "tentaflake remote-check "
            f"\"$(git -C {repository} remote get-url origin)\" "
            "https://github.com/example/allowed.git"
        )

    with subtest("credentials stay private and stable across service restarts"):
        credentials = "tentaflake-broker-credentials-zeroclaw-assistant.service"
        runtime_dir = "/run/tentaflake-broker/zeroclaw-assistant"
        broker = "tentaflake-broker-fetch-zeroclaw-assistant.service"
        machine.wait_for_unit(credentials)
        machine.succeed(f"test $(stat -c %a {runtime_dir}) = 700")
        for filename in ("agent-token", "agent.env"):
            machine.succeed(f"test $(stat -c %a {runtime_dir}/{filename}) = 400")
        token_digest = machine.succeed(f"sha256sum {runtime_dir}/agent-token")
        for transition in ("restart", "stop-start"):
            if transition == "restart":
                machine.succeed(f"systemctl restart {credentials}")
            else:
                machine.succeed(f"systemctl stop {credentials}")
                machine.succeed(f"systemctl start {credentials}")
            machine.wait_for_unit(credentials)
            assert machine.succeed(f"sha256sum {runtime_dir}/agent-token") == token_digest
            machine.succeed(f"test ! -e {runtime_dir}/agent.env.tmp")
            machine.succeed(f"systemctl restart {broker}")
            machine.wait_for_unit(broker)
            wait_for_broker_health(machine, "10.203.30.1:7811", f"journalctl -b -u {broker} -n 100")
        broker_pid = machine.succeed(f"systemctl show -p MainPID --value {broker}").strip()
        machine.succeed(f"grep -Eq '^CapBnd:[[:space:]]+0+$' /proc/{broker_pid}/status")
        for property_name, expected in (("MemoryMax", "134217728"), ("TasksMax", "64"), ("LimitNOFILE", "4096")):
            assert machine.succeed(f"systemctl show -p {property_name} --value {broker}").strip() == expected

    with subtest("fetch broker rejects unauthenticated and SSRF requests"):
        machine.succeed(
            "systemctl start "
            "tentaflake-broker-fetch-zeroclaw-assistant"
        )
        machine.wait_for_unit(
            "tentaflake-broker-fetch-zeroclaw-assistant.service"
        )
        wait_for_broker_health(
            machine,
            "10.203.30.1:7811",
            "systemctl --no-pager --full status "
            "tentaflake-broker-fetch-zeroclaw-assistant.service; "
            "journalctl --no-pager -b -u "
            "tentaflake-broker-fetch-zeroclaw-assistant.service -n 100",
        )
        broker_report = machine.succeed(
            "tentaflake doctor --security --json"
        )
        assert "TFSEC-035" not in broker_report, broker_report
        assert "TFSEC-036" not in broker_report, broker_report
        machine.succeed(
            "docker network inspect tf-zeroclaw-assistant | "
            "jq -e '.[0].Internal == true and "
            "(.[0].Options[\"com.docker.network.bridge.name\"] "
            "| startswith(\"tfb-\"))'"
        )
        machine.succeed(
            "nft list ruleset | "
            "grep -F 'iifname \"tfb-'"
        )
        unauth = machine.succeed(
            "curl -sS -o /tmp/unauth -w '%{http_code}' "
            "-H 'Content-Type: application/json' "
            "-d '{\"url\":\"https://example.com/\"}' "
            "http://10.203.30.1:7811/v1/fetch"
        ).strip()
        assert unauth == "401", unauth
        token = machine.succeed(
            "cat /run/tentaflake-broker/"
            "zeroclaw-assistant/agent-token"
        ).strip()
        ssrf = machine.succeed(
            "curl -sS -o /tmp/ssrf -w '%{http_code}' "
            f"-H 'Authorization: Bearer {token}' "
            "-H 'Content-Type: application/json' "
            "-d '{\"url\":"
            "\"https://169.254.169.254/latest/\"}' "
            "http://10.203.30.1:7811/v1/fetch"
        ).strip()
        assert ssrf == "403", ssrf

    with subtest("an agent can use the scoped LLM broker path"):
        machine.succeed(
            "docker load < "
            "/etc/tentaflake/network-test-image"
        )
        machine.succeed(
            "systemctl stop "
            "tentaflake-broker-fetch-zeroclaw-assistant.service"
        )
        machine.succeed(
            "printf '%s\\n' 'virtual-fixture-key' > "
            "/tmp/tentaflake-fixture-agent-token"
        )
        machine.succeed(
            "printf '%s\\n' 'fixture-provider-key' > "
            "/tmp/tentaflake-fixture-provider-token"
        )
        machine.succeed(
            "chmod 0600 /tmp/tentaflake-fixture-agent-token "
            "/tmp/tentaflake-fixture-provider-token"
        )
        machine.succeed(
            "cat > /tmp/tentaflake-fixture-broker.json <<'EOF'\n"
            "{\n"
            "  \"agent\": \"fixture\",\n"
            "  \"listen\": \"10.203.30.1:7811\",\n"
            "  \"token_file\": \"/tmp/tentaflake-fixture-agent-token\",\n"
            "  \"audit_file\": \"/tmp/tentaflake-fixture-audit.jsonl\",\n"
            "  \"budget_state_file\": \"/tmp/tentaflake-fixture-budget.json\",\n"
            "  \"llm\": {\n"
            "    \"upstream_base_url\": \"http://127.0.0.1:18080/v1/\",\n"
            "    \"provider_credential_file\": \"/tmp/tentaflake-fixture-provider-token\",\n"
            "    \"allow_plain_http_for_tests\": true,\n"
            "    \"streaming\": {\"enable\": true},\n"
            "    \"allowed_models\": [{\n"
            "      \"name\": \"fixture/model\",\n"
            "      \"input_microusd_per_million\": 1,\n"
            "      \"output_microusd_per_million\": 1\n"
            "    }]\n"
            "  }\n"
            "}\n"
            "EOF"
        )
        machine.succeed(
            "python3 /etc/tentaflake/llm-upstream.py "
            "> /tmp/tentaflake-fixture-upstream.log 2>&1 & "
            "echo $! > /tmp/tentaflake-fixture-upstream.pid"
        )
        machine.succeed(
            "tentaflake-broker --config /tmp/tentaflake-fixture-broker.json "
            "> /tmp/tentaflake-fixture-broker.log 2>&1 & "
            "echo $! > /tmp/tentaflake-fixture-broker.pid"
        )
        wait_for_broker_health(
            machine,
            "10.203.30.1:7811",
            "cat /tmp/tentaflake-fixture-broker.log; "
            "ps -ef | grep -F tentaflake-broker",
        )
        scoped_client = (
            "docker run --rm --runtime=runsc "
            "--network=tf-zeroclaw-assistant --dns=127.0.0.1 "
            "--read-only --user=65534:65534 --cap-drop=ALL "
            "--security-opt=no-new-privileges:true "
            "--security-opt=apparmor=docker-default "
            "--memory=64m --memory-swap=64m --cpus=0.5 "
            f"--pids-limit={runtime_probe_pids_limit} "
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=8m "
            "tentaflake-network-test:latest "
        )
        machine.succeed(
            scoped_client
            + "sh -c 'curl --fail --silent "
            + "-H \"Authorization: Bearer virtual-fixture-key\" "
            + "-H \"Content-Type: application/json\" "
            + "-H \"X-OpenCode-Session: conversation-vm\" "
            + "-H \"X-Forged: not-forwarded\" "
            + "-d \"{\\\"model\\\":\\\"fixture/model\\\","
            + "\\\"messages\\\":[],\\\"max_tokens\\\":1}\" "
            + "http://10.203.30.1:7811/v1/chat/completions "
            + "| grep -F completion-fixture'"
        )
        for route, terminal in [("chat/completions", "[DONE]"), ("responses", "response.completed")]:
            token_field = "max_tokens" if route == "chat/completions" else "max_output_tokens"
            payload = '{"model":"fixture/model","stream":true,"' + token_field + '":1}'
            output = machine.succeed(
                scoped_client
                + "curl --fail --silent --no-buffer --max-time 5 "
                + "-H 'Authorization: Bearer virtual-fixture-key' "
                + "-H 'Content-Type: application/json' "
                + "-H 'X-OpenCode-Session: conversation-vm' "
                + "-H 'X-Forged: not-forwarded' "
                + f"-d '{payload}' http://10.203.30.1:7811/v1/{route}"
            )
            assert "stream-first" in output, output
            assert terminal in output, output
        audit = machine.succeed("cat /tmp/tentaflake-fixture-audit.jsonl")
        assert '"streaming":true' in audit, audit
        assert "stream-first" not in audit, audit
        assert "virtual-fixture-key" not in audit, audit
        assert "fixture-provider-key" not in audit, audit
        assert "conversation-vm" not in audit, audit
        machine.succeed(
            "kill $(cat /tmp/tentaflake-fixture-broker.pid) "
            "$(cat /tmp/tentaflake-fixture-upstream.pid)"
        )
        machine.succeed(
            "systemctl start "
            "tentaflake-broker-fetch-zeroclaw-assistant.service"
        )
        machine.wait_for_unit(
            "tentaflake-broker-fetch-zeroclaw-assistant.service"
        )
        wait_for_broker_health(
            machine,
            "10.203.30.1:7811",
            "systemctl --no-pager --full status "
            "tentaflake-broker-fetch-zeroclaw-assistant.service; "
            "journalctl --no-pager -b -u "
            "tentaflake-broker-fetch-zeroclaw-assistant.service -n 100",
        )

    with subtest("an external node cannot reach agent or broker listeners"):
        for port in [3000, 42617, 7810, 7811, 8080]:
            attacker.fail(
                "curl --fail --silent --connect-timeout 2 "
                f"http://machine:{port}/"
            )

    with subtest("podman uses runsc and the exact internal bridge"):
        unit = unit_with_start_script(
            podman,
            "podman-zeroclaw-podman.service",
        )
        normalized_unit = " ".join(
            unit.replace(chr(92) + "\n", " ").split()
        )
        for flag in [
            "--runtime runsc",
            "--cap-drop=ALL",
            "--read-only",
            "--security-opt=no-new-privileges:true",
            "--memory=2g",
            "--memory-swap=2g",
            "--cpus=2.0",
            "--pids-limit=512",
        ]:
            assert flag in normalized_unit, f"missing {flag}:\n{unit}"
        podman.succeed(
            "systemctl start "
            "tentaflake-broker-network-zeroclaw-podman"
        )
        podman.succeed(
            "podman network inspect tf-zeroclaw-podman | "
            "jq -e '.[0].internal == true and "
            "(.[0].network_interface | startswith(\"tfb-\"))'"
        )
        podman.succeed(
            "podman load < /etc/tentaflake/podman-test-image"
        )
        check_live_doctor(podman, "podman", "tentaflake-podman-test:latest",
                          "tf-zeroclaw-podman", "10.203.50.1:7811")
        podman_base = (
            "podman run --rm --runtime runsc "
            "--network none --read-only "
            "--user 65534:65534 --cap-drop ALL "
            "--security-opt no-new-privileges "
            "--memory 64m --memory-swap 64m "
            f"--cpus 0.5 --pids-limit {runtime_probe_pids_limit} "
            "--tmpfs /tmp:rw,nosuid,nodev,noexec,size=8m "
            "tentaflake-podman-test:latest "
        )
        podman.succeed(
            podman_base + "sh -c 'test $(id -u) = 65534'"
        )
        podman.succeed(
            podman_base
            + "sh -c \"grep -q '^CapEff:[[:space:]]*0*' "
            + "/proc/self/status\""
        )
        podman.fail(podman_base + "sh -c 'touch /etc/deny'")
        podman.fail(podman_base + "sh -c 'touch /usr/deny'")
        podman.fail(
            podman_base
            + "curl --fail --connect-timeout 2 "
            + "https://1.1.1.1/"
        )

    with subtest("broker recovery preserves dependents and respects an explicit stop"):
        unit = "tentaflake-broker-fetch-zeroclaw-assistant.service"
        dependent = "tentaflake-broker-dependent-fixture.service"
        machine.succeed(
            f"systemd-run --unit={dependent} "
            f"--property=Requires={unit} --property=After={unit} "
            "/run/current-system/sw/bin/sleep infinity"
        )
        machine.wait_for_unit(dependent)
        dependent_pid = int(machine.succeed(f"systemctl show -p MainPID --value {dependent}").strip())
        assert dependent_pid > 0, dependent_pid
        policy = machine.succeed(
            f"systemctl cat {unit}"
        )
        assert "RestartSec=10s" in policy, policy
        assert "StartLimitIntervalSec=0" in policy, policy
        assert "RestartSteps=5" in policy, policy
        assert "RestartMaxDelaySec=1min" in policy, policy
        # Accelerate the same recovery policy; all six crashes still happen
        # inside the former five-start window without resetting its counters.
        machine.succeed(
            f"mkdir -p /run/systemd/system/{unit}.d; "
            "printf '%s\\n' '[Service]' 'RestartSec=100ms' "
            f"'RestartMaxDelaySec=200ms' > /run/systemd/system/{unit}.d/test.conf; "
            "systemctl daemon-reload"
        )
        for crash in range(6):
            pid = machine.succeed(f"systemctl show -p MainPID --value {unit}").strip()
            assert int(pid) > 0, pid
            machine.succeed(f"kill -9 {pid}")
            machine.wait_until_succeeds(
                f"test $(systemctl show -p NRestarts --value {unit}) -ge {crash + 1} "
                f"&& systemctl is-active --quiet {unit}",
                timeout=30,
            )
            wait_for_broker_health(machine, "10.203.30.1:7811", f"journalctl -u {unit} -n 50")
            machine.succeed(f"systemctl is-active --quiet {dependent}")
            observed_pid = int(machine.succeed(f"systemctl show -p MainPID --value {dependent}").strip())
            assert observed_pid == dependent_pid, (crash, dependent_pid, observed_pid)
        machine.succeed(f"systemctl stop {unit}")
        machine.sleep(1)
        machine.fail(f"systemctl is-active --quiet {unit}")
        machine.fail(f"systemctl is-active --quiet {dependent}")
        machine.succeed(f"rm /run/systemd/system/{unit}.d/test.conf; systemctl daemon-reload; systemctl start {unit}")
        wait_for_broker_health(
            machine,
            "10.203.30.1:7811",
            f"systemctl --no-pager --full status {unit}; "
            f"journalctl --no-pager -b -u {unit} -n 100",
        )
        machine.fail(f"systemctl is-active --quiet {dependent}")

    with subtest("internal capsule network blocks direct authority"):
        machine.succeed(
            "docker load < "
            "/etc/tentaflake/network-test-image"
        )
        machine.succeed(
            "python3 -c 'from http.server import BaseHTTPRequestHandler,HTTPServer; "
            "H=type(\"H\",(BaseHTTPRequestHandler,),{\"do_GET\":lambda s:("
            "s.send_response(200),s.end_headers(),s.wfile.write(b\"host-fixture\"))}); "
            "HTTPServer((\"127.0.0.1\",18081),H).serve_forever()' "
            "> /tmp/tentaflake-loopback-fixture.log 2>&1 & "
            "echo $! > /tmp/tentaflake-loopback-fixture.pid"
        )
        machine.succeed(
            "python3 -c 'from http.server import BaseHTTPRequestHandler,HTTPServer; "
            "H=type(\"H\",(BaseHTTPRequestHandler,),{\"do_GET\":lambda s:("
            "s.send_response(200),s.end_headers(),s.wfile.write(b\"gateway-fixture\"))}); "
            "HTTPServer((\"10.203.30.1\",18082),H).serve_forever()' "
            "> /tmp/tentaflake-gateway-fixture.log 2>&1 & "
            "echo $! > /tmp/tentaflake-gateway-fixture.pid"
        )
        base = (
            "docker run --rm "
            "--runtime=runsc "
            "--network=tf-zeroclaw-assistant "
            "--dns=127.0.0.1 "
            "--read-only "
            "--user=65534:65534 "
            "--cap-drop=ALL "
            "--security-opt=no-new-privileges:true "
            "--security-opt=apparmor=docker-default "
            "--memory=64m --memory-swap=64m "
            f"--cpus=0.5 --pids-limit={runtime_probe_pids_limit} "
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=8m "
            "--tmpfs=/workspace:rw,nosuid,nodev,size=8m "
            "tentaflake-network-test:latest "
        )
        machine.succeed(
            "docker run -d --name tf-resource-limits "
            "--runtime=runsc --network=none --read-only "
            "--user=65534:65534 --cap-drop=ALL "
            "--security-opt=no-new-privileges:true "
            "--security-opt=apparmor=docker-default "
            "--memory=64m --memory-swap=64m --cpus=0.5 "
            f"--pids-limit={runtime_probe_pids_limit} "
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=8m "
            "tentaflake-network-test:latest sleep 30"
        )
        machine.succeed(
            "docker inspect tf-resource-limits | jq -e "
            "'.[0].HostConfig.Memory == 67108864 and "
            ".[0].HostConfig.MemorySwap == 67108864 and "
            ".[0].HostConfig.NanoCpus == 500000000 and "
            f".[0].HostConfig.PidsLimit == {runtime_probe_pids_limit}'"
        )
        machine.succeed("docker rm --force tf-resource-limits")
        check_live_doctor(machine, "docker", "tentaflake-network-test:latest",
                          "tf-zeroclaw-assistant", "10.203.30.1:7811")
        machine.succeed(base + "sh -c 'test $(id -u) = 65534'")
        machine.succeed(
            base
            + "sh -c \"grep -q '^CapEff:[[:space:]]*0*' "
            + "/proc/self/status\""
        )
        machine.succeed(
            base + "sh -c 'touch /workspace/ok'"
        )
        machine.fail(
            base
            + "sh -c 'n=0; for i in $(seq 1 64); do "
            + "sleep 30 & status=$?; "
            + "test $status -eq 0 || break; "
            + "n=$((n + 1)); done; "
            + "test $n -eq 64'"
        )
        machine.fail(base + "sh -c 'touch /etc/deny'")
        machine.fail(base + "sh -c 'touch /usr/deny'")
        machine.fail(
            base
            + "sh -c 'dd if=/dev/zero "
            + "of=/workspace/too-big bs=1M count=16'"
        )
        machine.fail(
            base
            + "sh -c 'nslookup example.com 127.0.0.1'"
        )
        machine.fail(
            base
            + "sh -c 'nslookup example.com 1.1.1.1'"
        )
        machine.fail(
            base
            + "curl --fail --connect-timeout 2 "
            + "https://1.1.1.1/"
        )
        machine.fail(
            base
            + "curl --fail --connect-timeout 2 "
            + "http://127.0.0.1:18081/"
        )
        machine.fail(
            base
            + "curl --fail --connect-timeout 2 "
            + "http://10.203.30.1:18082/"
        )
        for target in [
            "127.0.0.1",
            "10.203.30.1",
            "172.16.0.1",
            "192.168.1.1",
            "100.100.100.100",
            "169.254.1.1",
            "169.254.169.254",
            "224.0.0.1",
        ]:
            machine.fail(
                base
                + "curl --fail --connect-timeout 2 "
                + f"http://{target}/"
            )
        broker_status = machine.succeed(
            base
            + "curl -sS -o /tmp/broker -w '%{http_code}' "
            + f"-H 'Authorization: Bearer {token}' "
            + "-H 'Content-Type: application/json' "
            + "-d '{\"url\":"
            + "\"https://169.254.169.254/latest/\"}' "
            + "http://10.203.30.1:7811/v1/fetch"
        ).strip()
        assert broker_status == "403", broker_status
        environment = machine.succeed(base + "env")
        assert "real-provider-key" not in environment
        assert "GH_TOKEN=" not in environment
        assert "DOCKER_HOST=" not in environment
        machine.succeed(
            base
            + "sh -c 'test ! -e /var/lib/hermes-test "
            + "-a ! -e /var/lib/zeroclaw-assistant "
            + "-a ! -e /run/docker.sock'"
        )
        machine.succeed(
            "kill $(cat /tmp/tentaflake-loopback-fixture.pid) "
            "$(cat /tmp/tentaflake-gateway-fixture.pid)"
        )

    with subtest("disposable worker enforces policy and approval outside the agent"):
        inbox = "/var/lib/hermes-test/workspace/.tentaflake-worker/inbox"
        results = "/var/lib/tentaflake-worker-hermes-test/results"
        machine.succeed("systemctl is-active tentaflake-worker-hermes-test.path")
        machine.succeed("systemctl is-active tentaflake-worker-image.service")
        machine.succeed("getent group tfw-gid-10000 | grep -F ':x:10000:'")
        machine.succeed(
            "test $(systemctl show -P Group "
            "tentaflake-worker-hermes-test.service) = tfw-gid-10000"
        )
        worker_policy = machine.succeed(
            "systemctl cat "
            "tentaflake-worker-hermes-test.service"
        )
        assert "CAP_DAC_READ_SEARCH" in worker_policy, worker_policy
        assert "CAP_CHOWN" not in worker_policy, worker_policy
        assert "StartLimitIntervalSec=0" in worker_policy, worker_policy
        assert "StartLimitBurst=" not in worker_policy, worker_policy
        machine.succeed(
            "test $(systemctl show -P Group "
            "tentaflake-worker-zeroclaw-assistant.service) = nogroup"
        )
        machine.succeed("test $(stat -c %a " + inbox + ") = 770")
        machine.succeed(
            "cat > " + inbox + "/local_1.json <<'EOF'\n"
            '{"version":1,"id":"local_1",'
            '"action_class":"local-reversible",'
            '"argv":["sh","-c",'
            '"mkdir artifacts; id -u > artifacts/uid; '
            'test ! -e /run/docker.sock; '
            '! wget -T 1 -q https://1.1.1.1/ -O /tmp/net"],'
            '"timeout_seconds":5}\nEOF'
        )
        result_file = results + "/local_1/result.json"
        worker_unit = "tentaflake-worker-hermes-test.service"
        wait_for_worker_file(machine, worker_unit, result_file)
        machine.succeed(
            "jq -e '.status == \"succeeded\" and "
            ".artifacts_available == true' "
            + result_file
            + " || { cat "
            + result_file
            + "; cat "
            + results
            + "/local_1/job.log; exit 1; }"
        )
        machine.succeed("test $(cat " + results + "/local_1/artifacts/uid) = 10000")
        machine.fail("docker inspect tfw-hermes-test-local_1")
        machine.fail("test -e /var/lib/tentaflake-worker-hermes-test/jobs/local_1")

        machine.succeed(
            "cat > " + inbox + "/timeout_1.json <<'EOF'\n"
            '{"version":1,"id":"timeout_1",'
            '"action_class":"local-reversible",'
            '"argv":["sh","-c","sleep 30"],'
            '"timeout_seconds":1}\nEOF'
        )
        wait_for_worker_file(
            machine,
            worker_unit,
            results + "/timeout_1/result.json",
        )
        machine.succeed(
            "jq -e '.status == \"timed-out\" and "
            ".timed_out == true' "
            + results + "/timeout_1/result.json"
        )
        machine.fail("docker inspect tfw-hermes-test-timeout_1")

        machine.succeed(
            "cat > " + inbox + "/message_1.json <<'EOF'\n"
            '{"version":1,"id":"message_1",'
            '"action_class":"communicative",'
            '"argv":["sh","-c","mkdir artifacts; echo approved > artifacts/state"],'
            '"timeout_seconds":5}\nEOF'
        )
        wait_for_worker_file(
            machine,
            worker_unit,
            "/var/lib/tentaflake-worker-hermes-test/pending/message_1.json",
        )
        machine.fail("test -e " + results + "/message_1")
        machine.succeed(
            "tentaflake-worker --config "
            "/etc/tentaflake/workers/hermes-test.json approve message_1"
        )
        machine.succeed(
            "jq -e '.status == \"succeeded\"' "
            + results
            + "/message_1/result.json || { cat "
            + results
            + "/message_1/result.json; cat "
            + results
            + "/message_1/job.log; exit 1; }"
        )
        machine.succeed(
            "grep -Fx approved "
            + results
            + "/message_1/artifacts/state"
        )

        machine.succeed(
            "cat > " + inbox + "/prompt_injection_1.json <<'EOF'\n"
            '{"version":1,"id":"prompt_injection_1",'
            '"action_class":"privileged",'
            '"argv":["sh","-c",'
            '"touch /var/lib/tentaflake-prompt-owned"],'
            '"timeout_seconds":5}\nEOF'
        )
        wait_for_worker_file(
            machine,
            worker_unit,
            "/var/lib/tentaflake-worker-hermes-test/"
            "pending/prompt_injection_1.json",
        )
        machine.fail("test -e /var/lib/tentaflake-prompt-owned")
        machine.succeed(
            "tentaflake-worker --config "
            "/etc/tentaflake/workers/hermes-test.json "
            "deny prompt_injection_1"
        )
        machine.succeed(
            "jq -e '.status == \"denied\"' "
            + results + "/prompt_injection_1/result.json"
        )
        machine.fail("test -e /var/lib/tentaflake-prompt-owned")

        machine.succeed(
            "cat > " + inbox + "/forbidden_1.json <<'EOF'\n"
            '{"version":1,"id":"forbidden_1",'
            '"action_class":"forbidden",'
            '"argv":["sh","-c","exit 0"],'
            '"timeout_seconds":5}\nEOF'
        )
        wait_for_worker_file(
            machine,
            worker_unit,
            results + "/forbidden_1/result.json",
        )
        machine.succeed("jq -e '.status == \"rejected\"' " + results + "/forbidden_1/result.json")

        audit = machine.succeed("cat /var/lib/tentaflake-worker-hermes-test/audit.jsonl")
        assert "approval-required" in audit, audit
        assert '"event":"approved"' in audit, audit

    with subtest("worker crash recovery never replays claimed or completed jobs"):
        state = "/var/lib/tentaflake-worker-hermes-test"
        inbox = "/var/lib/hermes-test/workspace/.tentaflake-worker/inbox"
        worker = "tentaflake-worker-hermes-test.service"
        watcher = "tentaflake-worker-hermes-test.path"
        command = "tentaflake-worker --config /etc/tentaflake/workers/hermes-test.json drain"
        machine.succeed("systemctl stop " + watcher + " " + worker)
        original = machine.succeed("sha256sum " + state + "/results/local_1/result.json")
        machine.succeed(
            "cat > " + state + "/pending/local_1.json <<'EOF'\n"
            '{"version":1,"id":"local_1","action_class":"local-reversible",'
            '"argv":["sh","-c","exit 99"],"timeout_seconds":5}\nEOF'
        )
        machine.succeed(command)
        assert machine.succeed("sha256sum " + state + "/results/local_1/result.json") == original
        machine.fail("test -e " + state + "/pending/local_1.json")
        machine.succeed("systemctl start " + watcher)
        machine.succeed(
            "cat > " + inbox + "/crash_1.json <<'EOF'\n"
            '{"version":1,"id":"crash_1","action_class":"local-reversible",'
            '"argv":["sh","-c","sleep 120"],"timeout_seconds":60}\nEOF'
        )
        machine.wait_until_succeeds(
            "test -f " + state + "/running/crash_1.json && "
            "test $(docker inspect --format='{{.State.Running}}' tfw-hermes-test-crash_1) = true",
            timeout=45,
        )
        machine.succeed("systemctl stop " + watcher)
        machine.succeed("systemctl kill --kill-who=main --signal=SIGKILL " + worker)
        machine.succeed("systemctl stop " + worker)
        machine.succeed("test -f " + state + "/running/crash_1.json")
        machine.succeed(command)
        machine.succeed("jq -e '.status == \"interrupted\" and .artifacts_available == false' " + state + "/results/crash_1/result.json")
        machine.fail("docker inspect tfw-hermes-test-crash_1")
        machine.fail("test -e " + state + "/running/crash_1.json")
        machine.fail("test -e " + state + "/jobs/crash_1")
        machine.succeed(command)
        machine.succeed("systemctl reset-failed " + worker)
        machine.succeed(
            "jq -e '.max_inbox_entries == 4 and .max_queue_entries == 2 and "
            ".max_queue_bytes == 1048576' /etc/tentaflake/workers/hermes-test.json"
        )
        for number in (1, 2, 3):
            machine.succeed(
                "cat > " + inbox + f"/capacity_{number}.json <<'EOF'\n"
                f'{{"version":1,"id":"capacity_{number}","action_class":"communicative",'
                '"argv":["sh","-c","exit 0"],"timeout_seconds":5}\nEOF'
            )
        machine.fail(command)
        machine.succeed("test -f " + state + "/pending/capacity_1.json")
        machine.succeed("test -f " + state + "/pending/capacity_2.json")
        machine.succeed("test -f " + inbox + "/capacity_3.json")
        machine.fail("test -e " + state + "/pending/capacity_3.json")
        operator = "tentaflake-worker --config /etc/tentaflake/workers/hermes-test.json "
        machine.succeed(operator + "deny capacity_1")
        machine.succeed(operator + "deny capacity_2")
        machine.succeed(command)
        machine.succeed(operator + "deny capacity_3")
        machine.succeed("grep -F '\"event\":\"queue-overload\"' " + state + "/audit.jsonl")
        machine.succeed("systemctl start " + watcher)

    with subtest("encrypted backup restores state and the mounted quota workspace"):
        machine.succeed(
            "install -d -m 0700 "
            "/run/tentaflake-backup"
        )
        machine.succeed(
            "printf '%s\\n' "
            "'/var/lib/tentaflake-test-restic' > "
            "/run/tentaflake-backup/repository"
        )
        machine.succeed(
            "printf '%s\\n' 'fixture-password' > "
            "/run/tentaflake-backup/password"
        )
        machine.succeed(
            "chmod 0400 "
            "/run/tentaflake-backup/repository "
            "/run/tentaflake-backup/password"
        )
        machine.succeed(
            "printf '%s\\n' 'restore-fixture' > "
            "/var/lib/hermes-test/restore-fixture"
        )
        machine.succeed(
            "printf '%s\\n' 'workspace-restore-fixture' > "
            "/var/lib/hermes-test/workspace/restore-fixture"
        )
        machine.succeed(
            "systemctl start "
            "restic-backups-tentaflake.service"
        )
        machine.wait_until_succeeds(
            "test -f "
            "/var/lib/tentaflake-backup/last-success || "
            "systemctl is-failed --quiet "
            "tentaflake-backup-success.service",
            timeout=30,
        )
        machine.succeed(
            "test -f "
            "/var/lib/tentaflake-backup/last-success || { "
            "systemctl status --no-pager "
            "tentaflake-backup-success.service; "
            "journalctl --no-pager -u "
            "tentaflake-backup-success.service; "
            "exit 1; }"
        )
        fresh_report = machine.succeed(
            "tentaflake doctor --security --json"
        )
        assert "TFSEC-027" not in fresh_report, fresh_report
        machine.succeed(
            "restic -r /var/lib/tentaflake-test-restic "
            "--password-file "
            "/run/tentaflake-backup/password "
            "snapshots --json | jq -e 'length == 1'"
        )
        machine.succeed(
            "restic -r /var/lib/tentaflake-test-restic "
            "--password-file "
            "/run/tentaflake-backup/password "
            "restore latest --target /tmp/restore"
        )
        restored = machine.succeed(
            "cat /tmp/restore/var/lib/hermes-test/"
            "restore-fixture"
        ).strip()
        assert restored == "restore-fixture", restored
        restored_workspace = machine.succeed(
            "cat /tmp/restore/var/lib/hermes-test/workspace/restore-fixture"
        ).strip()
        assert restored_workspace == "workspace-restore-fixture", restored_workspace

    with subtest("backup fails with an unavailable quota mount and recovers afterward"):
        machine.succeed(
            "cp -p /var/lib/tentaflake-backup/last-success /tmp/backup-last-success; "
            "mount_unit=$(systemd-escape --path --suffix=mount "
            "/var/lib/hermes-test/workspace); "
            "systemctl stop \"$mount_unit\"; "
            "mv /var/lib/tentaflake-workspace-volumes/hermes-test.img "
            "/tmp/hermes-test.img; "
            "mkdir /var/lib/tentaflake-workspace-volumes/hermes-test.img"
        )
        machine.fail("findmnt --mountpoint /var/lib/hermes-test/workspace")
        machine.fail("systemctl start restic-backups-tentaflake.service")
        machine.succeed(
            "test \"$(stat -c %y /var/lib/tentaflake-backup/last-success)\" = "
            "\"$(stat -c %y /tmp/backup-last-success)\""
        )
        machine.succeed(
            "restic -r /var/lib/tentaflake-test-restic "
            "--password-file /run/tentaflake-backup/password "
            "snapshots --json | jq -e 'length == 1'"
        )
        machine.succeed(
            "rmdir /var/lib/tentaflake-workspace-volumes/hermes-test.img; "
            "mv /tmp/hermes-test.img "
            "/var/lib/tentaflake-workspace-volumes/hermes-test.img; "
            "systemctl start restic-backups-tentaflake.service; "
            "systemctl start tentaflake-workspace-quota-hermes-test.service"
        )
        machine.succeed(
            "restic -r /var/lib/tentaflake-test-restic "
            "--password-file /run/tentaflake-backup/password "
            "snapshots --json | jq -e 'length == 2'"
        )

    with subtest("security doctor reports an intentionally unsafe fixture"):
        machine.succeed(
            "cat > /tmp/tentaflake-unsafe.tsv <<'EOF'\n"
            "host\tdev\ttrue\tfalse\tfalse\tfalse\tfalse\tfalse\t36\n"
            "agent\tcoding\tdev\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\t-\tfalse\t-\n"
            "EOF\n"
            "cat > /tmp/tentaflake-unsafe.conf <<'EOF'\n"
            "backend=docker\n"
            "flake_dir=/tmp\n"
            "host_name=unsafe-fixture\n"
            "agents_file=/etc/tentaflake/agents.tsv\n"
            "security_profile=dev\n"
            "security_file=/tmp/tentaflake-unsafe.tsv\n"
            "EOF"
        )
        doctor_status, _ = machine.execute(
            "TENTAFLAKE_CONFIG=/tmp/tentaflake-unsafe.conf "
            "tentaflake doctor --security --json "
            "> /tmp/tentaflake-unsafe.json"
        )
        assert doctor_status == 1, doctor_status
        machine.succeed(
            "jq -e '([.findings[].id] | index(\"TFSEC-001\")) != null "
            "and ([.findings[].id] | index(\"TFSEC-002\")) != null "
            "and ([.findings[].severity] | index(\"critical\")) != null' "
            "/tmp/tentaflake-unsafe.json"
        )

    with subtest("reboot restores the declared fail-closed services and limits"):
        boot_id = machine.succeed(
            "cat /proc/sys/kernel/random/boot_id"
        ).strip()
        # `machine.reboot()` only sends Ctrl-Alt-Delete, which kmscon consumes
        # before the guest can act on it. Ask systemd for the controlled reboot
        # and prepare the test driver's persistent virtio shell to reconnect.
        machine.execute("systemctl reboot >&2 &", check_return=False)
        machine.connected = False
        machine.wait_for_unit("multi-user.target")
        machine.wait_for_unit("docker.service")
        machine.succeed("test -L /var/run")
        machine.succeed("test -S /run/docker.sock")
        machine.succeed("test -S /var/run/docker.sock")
        failed_units = machine.succeed(
            "systemctl --failed --no-legend --plain"
        ).strip()
        assert failed_units == "", f"failed reboot units:\n{failed_units}"
        assert boot_id != machine.succeed(
            "cat /proc/sys/kernel/random/boot_id"
        ).strip()
        machine.wait_for_unit(
            "tentaflake-workspace-quota-hermes-test.service"
        )
        machine.wait_for_unit("tentaflake-state-quota-hermes-test.service")
        assert machine.succeed("cat /var/lib/hermes-test/restore-fixture").strip() == "restore-fixture"
        assert machine.succeed("cat /var/lib/hermes-test/workspace/restore-fixture").strip() == "workspace-restore-fixture"
        machine.wait_for_unit(
            "tentaflake-worker-hermes-test.path"
        )
        machine.wait_for_unit(
            "tentaflake-broker-fetch-zeroclaw-assistant.service"
        )
        wait_for_broker_health(
            machine,
            "10.203.30.1:7811",
            "systemctl --no-pager --full status "
            "tentaflake-broker-fetch-zeroclaw-assistant.service; "
            "journalctl --no-pager -b -u "
            "tentaflake-broker-fetch-zeroclaw-assistant.service -n 100",
        )
        assert "--network=none" in unit_with_start_script(
            machine, "docker-hermes-test.service"
        )
        assert "--runtime=runsc" in unit_with_start_script(
            machine, "docker-zeroclaw-assistant.service"
        )
        machine.succeed(
            "docker network inspect tf-zeroclaw-assistant | "
            "jq -e '.[0].Internal == true'"
        )

    with subtest("legacy ext4 state and workspace images fail without being reformatted"):
        for kind, image in [
            ("workspace", "/var/lib/tentaflake-workspace-volumes/hermes-test.img"),
            ("state", "/var/lib/tentaflake-workspace-volumes/state/hermes-test.img"),
        ]:
            machine.succeed(
                "mount_unit=$(systemd-escape --path --suffix=mount /var/lib/hermes-test/workspace); "
                "systemctl stop \"$mount_unit\""
            )
            if kind == "state":
                machine.succeed(
                    "mount_unit=$(systemd-escape --path --suffix=mount /var/lib/hermes-test); "
                    "systemctl stop \"$mount_unit\""
                )
            machine.succeed(
                f"mv {image} {image}.btrfs-saved; "
                f"truncate -s 128M {image}; mkfs.ext4 -F -q {image}"
            )
            before = machine.succeed(f"sha256sum {image}").split()[0]
            prepare = f"tentaflake-{kind}-quota-prepare-hermes-test.service"
            machine.fail(f"systemctl start {prepare}")
            assert machine.succeed(f"sha256sum {image}").split()[0] == before
            machine.succeed(f"blkid -p -s TYPE -o value {image} | grep -Fx ext4")
            machine.succeed(
                f"journalctl -u {prepare} --no-pager "
                f"| grep -F '{kind} image is not Btrfs'"
            )
            machine.succeed(
                f"rm {image}; mv {image}.btrfs-saved {image}; "
                f"systemctl reset-failed {prepare}; "
                "systemctl start tentaflake-workspace-quota-hermes-test.service"
            )
            machine.succeed("findmnt --mountpoint /var/lib/hermes-test -n -o FSTYPE | grep -Fx btrfs")
            machine.succeed("findmnt --mountpoint /var/lib/hermes-test/workspace -n -o FSTYPE | grep -Fx btrfs")
            assert machine.succeed("cat /var/lib/hermes-test/restore-fixture").strip() == "restore-fixture"
            assert machine.succeed("cat /var/lib/hermes-test/workspace/restore-fixture").strip() == "workspace-restore-fixture"

    with subtest("quota preparation refuses a source symlink without changing its target"):
        machine.succeed(
            "systemctl stop $(systemd-escape --path --suffix=mount /var/lib/hermes-test/workspace); "
            "systemctl stop $(systemd-escape --path --suffix=mount /var/lib/hermes-test); "
            "mv /var/lib/hermes-test /var/lib/hermes-test-offline; "
            "ln -s /var/lib/quota-unrelated /var/lib/hermes-test"
        )
        machine.fail("systemctl start tentaflake-state-quota-prepare-hermes-test.service")
        machine.succeed(
            "test $(stat -c %a /var/lib/quota-unrelated) = 755; "
            "test $(cat /var/lib/quota-unrelated/marker) = preserve; "
            "journalctl -u tentaflake-state-quota-prepare-hermes-test.service --no-pager "
            "| grep -F 'refusing symlink or non-directory quota source'"
        )
        machine.succeed(
            "rm /var/lib/hermes-test; mv /var/lib/hermes-test-offline /var/lib/hermes-test; "
            "printf '%s' preserve-unmounted > /var/lib/hermes-test/unmounted-state-note; "
            "systemctl reset-failed tentaflake-state-quota-prepare-hermes-test.service"
        )
        machine.fail("systemctl start tentaflake-state-quota-prepare-hermes-test.service")
        machine.succeed(
            "test $(cat /var/lib/hermes-test/unmounted-state-note) = preserve-unmounted; "
            "rm /var/lib/hermes-test/unmounted-state-note; "
            "systemctl reset-failed tentaflake-state-quota-prepare-hermes-test.service; "
            "systemctl start tentaflake-workspace-quota-hermes-test.service"
        )
        assert machine.succeed("cat /var/lib/hermes-test/restore-fixture").strip() == "restore-fixture"

  '';
}
