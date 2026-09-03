{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.worker;
  backend = config.virtualisation.oci-containers.backend;
  runtime = lib.getExe pkgs.${backend};
  json = pkgs.formats.json { };
  containerSecurity = import ../lib/containerSecurity.nix { inherit lib; };
  enabledAgents = lib.filterAttrs (_: agent: agent.enable) cfg.agents;
  enabledAgentCount = lib.length (lib.attrNames enabledAgents);
  containerNames = lib.attrNames config.virtualisation.oci-containers.containers;
  backendUnits = lib.optional (backend == "docker") "docker.service";
  utils = import (pkgs.path + "/nixos/lib/utils.nix") { inherit lib config pkgs; };
  maxLinuxId = 4294967294;
  maxMemoryBytes = 1024 * 1024 * 1024 * 1024;
  maxTmpfsBytes = 64 * 1024 * 1024 * 1024;
  maxCpus = 1024;
  maxPids = 65536;
  maxSnapshotBytes = 16 * 1024 * 1024 * 1024;
  maxSnapshotEntries = 2000000;
  maxLogBytes = 64 * 1024 * 1024;
  maxTimeoutSeconds = 24 * 60 * 60;
  sizeComponents =
    value:
    let
      matched = builtins.match "^([1-9][0-9]*)([kKmMgGtT]?)$" value;
      factors = {
        "" = 1;
        k = 1024;
        m = 1024 * 1024;
        g = 1024 * 1024 * 1024;
        t = 1024 * 1024 * 1024 * 1024;
      };
    in
    if matched == null || builtins.stringLength (lib.head matched) > 13 then
      null
    else
      {
        amount = builtins.fromJSON (lib.head matched);
        factor = factors.${lib.toLower (lib.elemAt matched 1)};
      };
  sizeBytes =
    value:
    let
      components = sizeComponents value;
    in
    if components == null then 0 else components.amount * components.factor;
  validSize =
    maximum: value:
    let
      components = sizeComponents value;
    in
    components != null && components.amount <= maximum / components.factor;
  validCpus =
    value:
    builtins.stringLength value <= 16
    && builtins.match "^([1-9][0-9]*([.][0-9]{1,5})?|0[.][0-9]{0,4}[1-9][0-9]{0,4})$" value != null
    && builtins.fromJSON value <= maxCpus;
  safeWorkspacePath =
    value:
    lib.hasPrefix "/var/lib/" value
    && containerSecurity.canonicalPath value
    && !(containerSecurity.forbiddenWritableSource value)
    && !(lib.hasPrefix "/var/lib/tentaflake-worker-" value);
  stateDir = name: "/var/lib/tentaflake-worker-${name}";
  resultDir = name: "${stateDir name}/results";
  stateVolumeRoot = "/var/lib/tentaflake-worker-state-volumes";
  stateImagePath = name: "${stateVolumeRoot}/${name}.img";
  statePrepareUnit = name: "tentaflake-worker-state-prepare-${name}.service";
  stateMountUnit = name: "${utils.escapeSystemdPath (stateDir name)}.mount";
  stateLayoutUnit = name: "tentaflake-worker-state-${name}.service";
  stateVolumeBytes = agent: agent.stateVolumeMiB * 1024 * 1024;
  totalStateVolumeMiB = lib.foldl' (total: agent: total + agent.stateVolumeMiB) 0 (
    lib.attrValues enabledAgents
  );
  totalWorkerMemoryBytes = lib.foldl' (total: agent: total + sizeBytes agent.memory) 0 (
    lib.attrValues enabledAgents
  );
  totalWorkerPids = lib.foldl' (total: agent: total + agent.pidsLimit) 0 (
    lib.attrValues enabledAgents
  );
  statePendingMetadataBytes = agent: agent.maxPendingRequests * 4096;
  stateRequiredBytes =
    agent:
    2 * agent.maxSnapshotBytes
    + agent.maxPendingBytes
    + agent.maxLogBytes
    + statePendingMetadataBytes agent
    + 16 * 1024 * 1024;
  # The image is formatted with mkfs.ext4 -m 0 -i 16384; retain 12.5% of
  # the raw image for its journal and allocation metadata before comparing
  # logical worker budgets with usable blocks.
  stateFilesystemUsableBytes = agent: (stateVolumeBytes agent) * 7 / 8;
  stateRequiredInodes = agent: 2 * agent.maxSnapshotEntries + agent.maxPendingRequests * 4 + 1024;
  stateAvailableInodes = agent: agent.stateVolumeMiB * 64;
  workerGroup =
    agent:
    if agent.hostGroup != null then
      agent.hostGroup
    else if agent.containerGid == 65534 then
      "nogroup"
    else
      "tfw-gid-${toString agent.containerGid}";
  autoWorkerGroups = lib.foldl' (
    groups: agent:
    let
      group = workerGroup agent;
    in
    if agent.hostGroup == null && agent.containerGid != 65534 then
      groups // { ${group}.gid = agent.containerGid; }
    else
      groups
  ) { } (lib.attrValues enabledAgents);

  agentType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        enable = lib.mkEnableOption "disposable no-egress tool worker for ${name}";
        workspace = lib.mkOption {
          type = lib.types.str;
          default = "";
          example = "/var/lib/hermes-coding/workspace";
          description = "Exact persistent controller workspace copied into bounded worker snapshots.";
        };
        containerUid = lib.mkOption {
          type = lib.types.ints.between 1 maxLinuxId;
          default = 10000;
        };
        containerGid = lib.mkOption {
          type = lib.types.ints.between 1 maxLinuxId;
          default = 10000;
        };
        hostGroup = lib.mkOption {
          type = lib.types.nullOr (lib.types.strMatching "^[a-z_][a-z0-9_-]{0,30}$");
          default = null;
          description = ''
            Existing host group whose explicit GID matches containerGid. Null
            creates one deterministic group per non-nogroup container GID.
          '';
        };
        maxRequestBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 64 * 1024;
        };
        maxPendingRequests = lib.mkOption {
          type = lib.types.ints.between 1 4096;
          default = 128;
          description = "Maximum number of root-owned requests awaiting execution or approval.";
        };
        maxPendingBytes = lib.mkOption {
          type = lib.types.ints.between 65536 (512 * 1024 * 1024);
          default = 8 * 1024 * 1024;
          description = "Maximum aggregate bytes reserved in the private pending queue.";
        };
        maxReadyJobsPerDrain = lib.mkOption {
          type = lib.types.ints.between 1 128;
          default = 1;
          description = "Maximum ready or forbidden requests completed by one queue activation.";
        };
        stateVolumeMiB = lib.mkOption {
          type = lib.types.ints.between 64 65536;
          default = 8192;
          description = ''
            Immutable ext4 image size for all private worker state, including
            pending requests, snapshots, result staging, retained results, and audit.
            Resizing requires an explicit offline migration.
          '';
        };
        maxSnapshotBytes = lib.mkOption {
          type = lib.types.ints.between 1 maxSnapshotBytes;
          default = 1024 * 1024 * 1024;
          description = "Maximum workspace snapshot bytes; at most 16 GiB.";
        };
        maxSnapshotEntries = lib.mkOption {
          type = lib.types.ints.between 1 maxSnapshotEntries;
          default = 200000;
          description = "Maximum workspace snapshot entries; at most 2,000,000.";
        };
        maxLogBytes = lib.mkOption {
          type = lib.types.ints.between 1 maxLogBytes;
          default = 1024 * 1024;
          description = "Maximum retained worker log bytes; at most 64 MiB.";
        };
        maxTimeoutSeconds = lib.mkOption {
          type = lib.types.ints.between 1 maxTimeoutSeconds;
          default = 900;
          description = "Maximum capsule runtime; bounded to 24 hours.";
        };
        memory = lib.mkOption {
          type = lib.types.str;
          default = "2g";
          description = "Hard capsule memory limit in OCI byte-size syntax; at most 1 TiB.";
        };
        memorySwap = lib.mkOption {
          type = lib.types.str;
          default = "2g";
          description = ''
            Total capsule memory-plus-swap limit in OCI byte-size syntax; at
            most 1 TiB. Setting it equal to memory disables additional swap.
          '';
        };
        cpus = lib.mkOption {
          type = lib.types.str;
          default = "2.0";
          description = "Fractional capsule CPU quota in OCI decimal syntax; at most 1024.";
        };
        pidsLimit = lib.mkOption {
          type = lib.types.ints.between 1 maxPids;
          default = 512;
          description = "Maximum number of process IDs available inside one capsule; at most 65,536.";
        };
        workspaceTmpfsSize = lib.mkOption {
          type = lib.types.str;
          default = "2g";
          description = "Hard tmpfs ceiling for the disposable writable workspace.";
        };
        tmpTmpfsSize = lib.mkOption {
          type = lib.types.str;
          default = "256m";
          description = "Hard tmpfs ceiling for the disposable capsule's /tmp; at most 64 GiB.";
        };
      };
    }
  );

  statePrepareService = name: agent: {
    description = "Prepare fixed-size private worker state filesystem for ${name}";
    requires = [ "systemd-tmpfiles-setup.service" ];
    after = [ "systemd-tmpfiles-setup.service" ];
    before = [ (stateMountUnit name) ];
    serviceConfig = {
      Type = "oneshot";
      UMask = "0077";
      NoNewPrivileges = true;
      PrivateDevices = false;
      PrivateTmp = true;
      ProtectHome = true;
      ProtectSystem = "strict";
      ReadWritePaths = [
        stateVolumeRoot
        (stateDir name)
      ];
      MemoryMax = 256 * 1024 * 1024;
      TasksMax = 32;
      CPUQuota = "25%";
      Nice = 10;
    };
    path = [
      pkgs.coreutils
      pkgs.e2fsprogs
      pkgs.findutils
      pkgs.util-linux
    ];
    script = ''
      image=${lib.escapeShellArg (stateImagePath name)}
      image_tmp=${lib.escapeShellArg "${stateImagePath name}.new"}
      state=${lib.escapeShellArg (stateDir name)}
      expected=$(( ${toString agent.stateVolumeMiB} * 1024 * 1024 ))

      install -d -m 0700 ${lib.escapeShellArg stateVolumeRoot}
      if [ -L "$state" ] || { [ -e "$state" ] && [ ! -d "$state" ]; }; then
        echo "tentaflake: worker state mountpoint is unsafe: $state" >&2
        exit 1
      fi
      install -d -m 0750 "$state"

      if [ -L "$image" ] || { [ -e "$image" ] && [ ! -f "$image" ]; }; then
        echo "tentaflake: worker state image is not a regular file: $image" >&2
        exit 1
      fi
      if [ -e "$image_tmp" ]; then
        echo "tentaflake: incomplete worker state image exists: $image_tmp" >&2
        echo "inspect and remove that exact file before retrying" >&2
        exit 1
      fi

      if [ ! -e "$image" ]; then
        if [ -n "$(find "$state" -mindepth 1 -print -quit)" ]; then
          echo "tentaflake: refusing to hide non-empty worker state $state" >&2
          echo "migrate it explicitly before enabling the fixed-size state volume" >&2
          exit 1
        fi
        fallocate --length "$expected" "$image_tmp"
        mkfs.ext4 -F -q -m 0 -i 16384 "$image_tmp"
        chmod 0600 "$image_tmp"
        mv -T "$image_tmp" "$image"
      fi

      actual=$(stat -c %s "$image")
      if [ "$actual" -ne "$expected" ]; then
        echo "tentaflake: worker state image size drift for ${name}" >&2
        echo "configured=$expected actual=$actual; use an explicit offline resize migration" >&2
        exit 1
      fi

      if ! findmnt --noheadings --mountpoint "$state" >/dev/null; then
        if ! fallocate --length "$expected" "$image"; then
          echo "tentaflake: cannot fully preallocate worker state image $image" >&2
          echo "free host disk space or use a filesystem supporting fallocate before retrying" >&2
          exit 1
        fi
        rc=0
        e2fsck -p "$image" || rc=$?
        if [ "$rc" -gt 1 ]; then
          echo "tentaflake: e2fsck failed for $image with status $rc" >&2
          exit "$rc"
        fi
      fi
    '';
  };

  stateLayoutService =
    name: agent:
    let
      group = workerGroup agent;
    in
    {
      description = "Initialize private worker state layout for ${name}";
      requires = [ (stateMountUnit name) ];
      bindsTo = [ (stateMountUnit name) ];
      after = [ (stateMountUnit name) ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        UMask = "0027";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectHome = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ (stateDir name) ];
        CapabilityBoundingSet = [
          "CAP_CHOWN"
          "CAP_DAC_OVERRIDE"
          "CAP_FOWNER"
          "CAP_FSETID"
        ];
      };
      path = [ pkgs.coreutils ];
      script = ''
        state=${lib.escapeShellArg (stateDir name)}
        for leaf in pending inflight jobs; do
          path="$state/$leaf"
          if [ -L "$path" ] || { [ -e "$path" ] && [ ! -d "$path" ]; }; then
            echo "tentaflake: worker state path is unsafe: $path" >&2
            exit 1
          fi
          install -d -m 0700 -o root -g root "$path"
        done

        results="$state/results"
        if [ -L "$results" ] || { [ -e "$results" ] && [ ! -d "$results" ]; }; then
          echo "tentaflake: worker result path is unsafe: $results" >&2
          exit 1
        fi
        install -d -m 2750 -o root -g ${lib.escapeShellArg group} "$results"
        marker="$state/.tentaflake-worker-state-v1"
        if [ -L "$marker" ] || { [ -e "$marker" ] && [ ! -f "$marker" ]; }; then
          echo "tentaflake: worker state marker is unsafe: $marker" >&2
          exit 1
        fi
        printf '%s\n' 'tentaflake-worker-state-v1' > "$marker"
        chown root:root "$marker"
        chmod 0600 "$marker"
        chown ${lib.escapeShellArg "root:${group}"} "$state"
        chmod 0750 "$state"
      '';
    };

  workerConfig =
    name: agent:
    json.generate "tentaflake-worker-${name}.json" {
      agent = name;
      inherit backend runtime;
      image = cfg.imageReference;
      inherit (agent) workspace;
      state_dir = stateDir name;
      container_uid = agent.containerUid;
      container_gid = agent.containerGid;
      max_request_bytes = agent.maxRequestBytes;
      max_pending_requests = agent.maxPendingRequests;
      max_pending_bytes = agent.maxPendingBytes;
      max_ready_jobs_per_drain = agent.maxReadyJobsPerDrain;
      max_snapshot_bytes = agent.maxSnapshotBytes;
      max_snapshot_entries = agent.maxSnapshotEntries;
      max_log_bytes = agent.maxLogBytes;
      max_timeout_seconds = agent.maxTimeoutSeconds;
      inherit (agent) memory cpus;
      memory_swap = agent.memorySwap;
      pids_limit = agent.pidsLimit;
      workspace_tmpfs_size = agent.workspaceTmpfsSize;
      tmp_tmpfs_size = agent.tmpTmpfsSize;
    };

  workerService =
    name: agent:
    let
      quota = lib.attrByPath [ name ] null config.tentaflake.workspaceQuota.agents;
      quotaUnits = lib.optional (
        quota != null && quota.enable
      ) "tentaflake-workspace-quota-${name}.service";
    in
    {
      description = "Disposable tool-worker queue for ${name}";
      wantedBy = [ "multi-user.target" ];
      requires = [
        "tentaflake-worker-image.service"
        (stateLayoutUnit name)
      ]
      ++ backendUnits
      ++ quotaUnits;
      after = [
        "tentaflake-worker-image.service"
        (stateLayoutUnit name)
      ]
      ++ backendUnits
      ++ quotaUnits;
      bindsTo = [ (stateLayoutUnit name) ];
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        Group = workerGroup agent;
        UMask = "0027";
        ExecStart = "${lib.getExe cfg.package} --config ${workerConfig name agent} drain";
        Restart = "on-failure";
        RestartSec = "10s";
        # A systemd start timeout must never terminate a claimed job midway:
        # its per-request capsule timeout remains bounded, while an interrupted
        # claim is terminalized as outcome-unknown instead of replayed.
        TimeoutStartSec = "infinity";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectSystem = "strict";
        ReadOnlyPaths = [ agent.workspace ];
        ReadWritePaths = [
          (stateDir name)
          "${agent.workspace}/.tentaflake-worker/inbox"
        ];
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
        SystemCallArchitectures = "native";
        MemoryMax = 256 * 1024 * 1024;
        TasksMax = 64;
        CPUQuota = "50%";
        LimitNOFILE = 4096;
        Nice = 10;
      };
      # Each start advances a bounded raw inbox scan and accepts a bounded
      # batch of eligible regular requests into a count- and byte-capped private
      # queue. It then completes a bounded number of ready jobs. The worker
      # validates its persisted cursor against a directory mutation stamp and
      # restarts safely from the beginning when that checkpoint is stale. An
      # intentionally non-zero continuation result makes Restart=on-failure
      # continue the next slice; a small aggregate start limit would permanently
      # disable this controlled backpressure path. Actual worker failures use
      # the same rate-bound retry interval.
      startLimitIntervalSec = 0;
    };

  allServices = lib.mapAttrs' (
    name: agent: lib.nameValuePair "tentaflake-worker-${name}" (workerService name agent)
  ) enabledAgents;
  stateServices = lib.foldlAttrs (
    services: name: agent:
    services
    // {
      "tentaflake-worker-state-prepare-${name}" = statePrepareService name agent;
      "tentaflake-worker-state-${name}" = stateLayoutService name agent;
      "tentaflake-worker-result-cleanup-${name}" = stateCleanupService name agent;
    }
  ) { } enabledAgents;
  stateMounts = lib.mapAttrsToList (name: _: {
    description = "Fixed-size private worker state for ${name}";
    what = stateImagePath name;
    where = stateDir name;
    type = "ext4";
    options = "loop,nodev,nosuid,noexec,noatime";
    requires = [ (statePrepareUnit name) ];
    after = [
      "local-fs.target"
      (statePrepareUnit name)
    ];
    before = [
      (stateLayoutUnit name)
      "umount.target"
    ];
    conflicts = [ "umount.target" ];
    wantedBy = [ "multi-user.target" ];
    # As with the managed workspace image, the preparation service must run
    # after ordinary local filesystems before this loop mount is activated.
    unitConfig.DefaultDependencies = false;
  }) enabledAgents;
  stateCleanupService = name: _: {
    description = "Expire retained worker results for ${name}";
    requires = [ (stateLayoutUnit name) ];
    bindsTo = [ (stateLayoutUnit name) ];
    after = [ (stateLayoutUnit name) ];
    serviceConfig = {
      Type = "oneshot";
      UMask = "0077";
      NoNewPrivileges = true;
      PrivateDevices = true;
      PrivateTmp = true;
      ProtectClock = true;
      ProtectControlGroups = true;
      ProtectHome = true;
      ProtectHostname = true;
      ProtectKernelLogs = true;
      ProtectKernelModules = true;
      ProtectKernelTunables = true;
      ProtectSystem = "strict";
      ReadWritePaths = [ (stateDir name) ];
      RestrictAddressFamilies = [ "AF_UNIX" ];
      RestrictNamespaces = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      SystemCallArchitectures = "native";
      CapabilityBoundingSet = [ "CAP_DAC_OVERRIDE" ];
      TimeoutStartSec = "5min";
      MemoryMax = 128 * 1024 * 1024;
      TasksMax = 32;
      CPUQuota = "25%";
      Nice = 10;
    };
    path = [
      pkgs.coreutils
      pkgs.findutils
    ];
    script = ''
      results=${lib.escapeShellArg (resultDir name)}
      if [ -L "$results" ] || [ ! -d "$results" ]; then
        echo "tentaflake: worker result root is unsafe: $results" >&2
        exit 1
      fi
      find "$results" -xdev -mindepth 1 -maxdepth 1 -type d -mtime +13 -exec \
        find -- {} -xdev -depth -ignore_readdir_race -delete \;
    '';
  };
  stateTimers = lib.mapAttrs' (
    name: _:
    lib.nameValuePair "tentaflake-worker-result-cleanup-${name}" {
      wantedBy = [ "timers.target" ];
      requires = [ (stateLayoutUnit name) ];
      after = [ (stateLayoutUnit name) ];
      bindsTo = [ (stateLayoutUnit name) ];
      timerConfig = {
        OnCalendar = "daily";
        RandomizedDelaySec = "6h";
        FixedRandomDelay = true;
        Persistent = true;
        Unit = "tentaflake-worker-result-cleanup-${name}.service";
      };
    }
  ) enabledAgents;
  allPaths = lib.mapAttrs' (
    name: agent:
    let
      quota = lib.attrByPath [ name ] null config.tentaflake.workspaceQuota.agents;
      quotaUnits = lib.optional (
        quota != null && quota.enable
      ) "tentaflake-workspace-quota-${name}.service";
    in
    lib.nameValuePair "tentaflake-worker-${name}" {
      description = "Watch the ${name} tool-worker inbox";
      wantedBy = [ "multi-user.target" ];
      requires = quotaUnits ++ [ (stateLayoutUnit name) ];
      after = [
        "basic.target"
        "systemd-tmpfiles-setup.service"
        (stateLayoutUnit name)
      ]
      ++ quotaUnits;
      bindsTo = [ (stateLayoutUnit name) ];
      before = [ "shutdown.target" ];
      conflicts = [ "shutdown.target" ];
      # A path unit normally starts before paths.target (and therefore before
      # basic.target). Quota-backed inboxes are late mounts, so start this
      # watcher explicitly after the ownership unit instead.
      unitConfig.DefaultDependencies = false;
      # Ignored inbox entries remain by design, so DirectoryNotEmpty would
      # continuously reactivate an otherwise converged oneshot.
      pathConfig = {
        PathChanged = "${agent.workspace}/.tentaflake-worker/inbox";
        Unit = "tentaflake-worker-${name}.service";
      };
    }
  ) enabledAgents;
in
{
  options.tentaflake.worker = {
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-worker { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/tentaflake-worker { }";
      description = "Host-side disposable worker orchestrator.";
    };
    image = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-worker/image.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/tentaflake-worker/image.nix { }";
      description = "Nix-built OCI image loaded locally for disposable jobs.";
    };
    imageReference = lib.mkOption {
      type = lib.types.str;
      default = "tentaflake-worker:0.4.0";
      description = "Exact local reference emitted by worker.image; this is not pulled from a registry.";
    };
    maxEnabledAgents = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = ''
        Optional host-wide ceiling for enabled disposable workers. Set this to
        the reviewed service, mount, and timer operating budget; evaluation
        rejects configurations exceeding it.
      '';
    };
    maxTotalStateVolumeMiB = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = ''
        Optional host-wide ceiling for the sum of enabled workers' fixed state
        image sizes. Set this to the reviewed disk budget; evaluation rejects
        configurations exceeding it.
      '';
    };
    maxTotalMemoryBytes = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = ''
        Optional host-wide ceiling for the sum of enabled workers' OCI memory
        limits. Set this to a reviewed capacity budget; evaluation rejects
        configurations exceeding it.
      '';
    };
    maxTotalPids = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = ''
        Optional host-wide ceiling for the sum of enabled workers' OCI PID
        limits. Set this below the reviewed host PID capacity; evaluation
        rejects configurations exceeding it.
      '';
    };
    agents = lib.mkOption {
      type = lib.types.attrsOf agentType;
      default = { };
      description = "Per-controller disposable worker and approval policy.";
    };
  };

  config = {
    assertions = [
      {
        assertion = enabledAgents == { } || config.tentaflake.security.profile != "dev";
        message = "tentaflake disposable workers require balanced or strict security profile.";
      }
      {
        assertion = enabledAgents == { } || config.tentaflake.hardening.enable;
        message = "tentaflake disposable workers require host hardening.";
      }
      {
        assertion =
          enabledAgents == { }
          || (cfg.imageReference != "" && lib.match "^[A-Za-z0-9._/:+-]+$" cfg.imageReference != null);
        message = "tentaflake worker imageReference must be one exact shell-safe local image reference.";
      }
      {
        assertion = cfg.maxEnabledAgents == null || enabledAgentCount <= cfg.maxEnabledAgents;
        message = "tentaflake worker enabled-agent count exceeds maxEnabledAgents.";
      }
      {
        assertion = cfg.maxTotalStateVolumeMiB == null || totalStateVolumeMiB <= cfg.maxTotalStateVolumeMiB;
        message = "tentaflake worker state-image total exceeds maxTotalStateVolumeMiB.";
      }
      {
        assertion = cfg.maxTotalMemoryBytes == null || totalWorkerMemoryBytes <= cfg.maxTotalMemoryBytes;
        message = "tentaflake worker memory-limit total exceeds maxTotalMemoryBytes.";
      }
      {
        assertion = cfg.maxTotalPids == null || totalWorkerPids <= cfg.maxTotalPids;
        message = "tentaflake worker PID-limit total exceeds maxTotalPids.";
      }
    ]
    ++ lib.concatLists (
      lib.mapAttrsToList (
        name: agent:
        let
          group = workerGroup agent;
        in
        [
          {
            assertion = lib.elem name containerNames;
            message = "tentaflake worker key ${name} must exactly match an OCI agent container name.";
          }
          {
            assertion = lib.match "^[a-z0-9][a-z0-9-]{0,47}$" name != null;
            message = "tentaflake worker names must be lowercase safe OCI identifiers up to 48 characters.";
          }
          {
            assertion = safeWorkspacePath agent.workspace;
            message = "tentaflake worker ${name} requires a canonical workspace below /var/lib and outside sensitive worker, runtime, or backing-volume roots.";
          }
          {
            assertion = validSize maxMemoryBytes agent.memory && validSize maxMemoryBytes agent.memorySwap;
            message = "tentaflake worker ${name} memory and memorySwap must be positive OCI byte sizes no larger than 1 TiB.";
          }
          {
            assertion =
              !(validSize maxMemoryBytes agent.memory && validSize maxMemoryBytes agent.memorySwap)
              || sizeBytes agent.memorySwap >= sizeBytes agent.memory;
            message = "tentaflake worker ${name} memorySwap must be greater than or equal to memory.";
          }
          {
            assertion = validCpus agent.cpus;
            message = "tentaflake worker ${name} cpus must be a positive decimal value no greater than 1024.";
          }
          {
            assertion =
              validSize maxTmpfsBytes agent.workspaceTmpfsSize && validSize maxTmpfsBytes agent.tmpTmpfsSize;
            message = "tentaflake worker ${name} tmpfs sizes must be positive OCI byte sizes no larger than 64 GiB.";
          }
          {
            assertion = agent.maxSnapshotBytes <= 16 * 1024 * 1024 * 1024;
            message = "tentaflake worker ${name} snapshot limit may not exceed 16 GiB.";
          }
          {
            assertion = (stateFilesystemUsableBytes agent) >= (stateRequiredBytes agent);
            message = "tentaflake worker ${name} stateVolumeMiB must reserve two snapshots, pending bytes and file blocks, logs, control space, and ext4 overhead.";
          }
          {
            assertion = (stateAvailableInodes agent) >= (stateRequiredInodes agent);
            message = "tentaflake worker ${name} stateVolumeMiB must reserve ext4 inodes for two bounded snapshots, pending metadata, and recovery.";
          }
          {
            assertion = agent.maxPendingBytes >= agent.maxRequestBytes;
            message = "tentaflake worker ${name} maxPendingBytes must fit one maxRequestBytes request.";
          }
          {
            assertion = agent.maxReadyJobsPerDrain <= agent.maxPendingRequests;
            message = "tentaflake worker ${name} maxReadyJobsPerDrain may not exceed maxPendingRequests.";
          }
          {
            assertion =
              lib.hasAttr group config.users.groups && config.users.groups.${group}.gid == agent.containerGid;
            message = "tentaflake worker ${name} host group ${group} must exist with GID ${toString agent.containerGid}.";
          }
        ]
      ) enabledAgents
    );

    environment.systemPackages = lib.optionals (enabledAgents != { }) [ cfg.package ];

    users.groups = autoWorkerGroups;

    environment.etc = lib.mapAttrs' (
      name: agent:
      lib.nameValuePair "tentaflake/workers/${name}.json" {
        source = workerConfig name agent;
        mode = "0444";
      }
    ) enabledAgents;

    services.logrotate.settings = lib.mapAttrs' (
      name: _:
      lib.nameValuePair "tentaflake-worker-${name}" {
        files = [ "${stateDir name}/audit.jsonl" ];
        frequency = "daily";
        rotate = 14;
        compress = true;
        delaycompress = true;
        missingok = true;
        notifempty = true;
        create = "0600 root root";
      }
    ) enabledAgents;

    systemd = {
      tmpfiles.rules =
        lib.optionals (enabledAgents != { }) [ "d ${stateVolumeRoot} 0700 root root -" ]
        ++ lib.concatLists (
          lib.mapAttrsToList (
            name: agent:
            let
              group = workerGroup agent;
            in
            [
              "d ${stateDir name} 0750 root root -"
              "d ${agent.workspace}/.tentaflake-worker 0700 ${toString agent.containerUid} ${group} -"
              "d ${agent.workspace}/.tentaflake-worker/inbox 0770 ${toString agent.containerUid} ${group} -"
            ]
          ) enabledAgents
        );

      mounts = stateMounts;
      services =
        stateServices
        // allServices
        // lib.optionalAttrs (enabledAgents != { }) {
          tentaflake-worker-image = {
            description = "Load the Nix-built tentaflake disposable worker image";
            wantedBy = [ "multi-user.target" ];
            requires = backendUnits;
            after = backendUnits;
            before = lib.mapAttrsToList (name: _: "tentaflake-worker-${name}.service") enabledAgents;
            serviceConfig = {
              Type = "oneshot";
              RemainAfterExit = true;
              ExecStart = "${runtime} load --input ${cfg.image}";
            };
          };
        };
      paths = allPaths;
      timers = stateTimers;
    };
  };
}
