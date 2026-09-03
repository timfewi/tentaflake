{ lib }:
let
  isSecure = profile: profile != "dev";

  pathWithin = root: path: path == root || lib.hasPrefix "${root}/" path;
  canonicalPath =
    path:
    lib.hasPrefix "/" path
    && lib.match "^/[A-Za-z0-9._+/-]+$" path != null
    && !(lib.elem "." (lib.splitString "/" path))
    && !(lib.elem ".." (lib.splitString "/" path))
    && !(lib.hasInfix "//" path)
    && !(lib.hasSuffix "/" path);
  forbiddenWritableSource = path: lib.any (root: pathWithin root path) sensitiveSources;
  statePathIsSafe =
    name: path:
    canonicalPath path
    && lib.hasPrefix "/var/lib/" path
    && lib.hasSuffix "/${name}" path
    && !(forbiddenWritableSource path)
    && !(lib.hasPrefix "/var/lib/tentaflake-worker-" path);
  volumeParts = volume: lib.splitString ":" volume;
  volumeSource = volume: lib.head (volumeParts volume);
  volumeDestination = volume: lib.elemAt (volumeParts volume) 1;
  volumeMode =
    volume:
    let
      parts = volumeParts volume;
    in
    if lib.length parts == 2 then
      "rw"
    else if lib.length parts == 3 then
      lib.elemAt parts 2
    else
      null;
  volumeSyntaxIsSafe =
    volume:
    lib.elem (volumeMode volume) [
      "rw"
      "ro"
    ];
  volumeIsReadOnly = volume: volumeMode volume == "ro";

  sensitiveSources = [
    "/"
    "/boot"
    "/dev"
    "/etc"
    "/home"
    "/proc"
    "/root"
    "/run"
    "/sys"
    "/var/lib/containers"
    "/var/lib/docker"
    "/var/lib/tentaflake-worker-state-volumes"
    "/var/lib/tentaflake-workspace-volumes"
    "/var/run"
  ];
  sensitiveDestinations = [
    "/"
    "/bin"
    "/boot"
    "/dev"
    "/etc"
    "/lib"
    "/lib64"
    "/proc"
    "/root"
    "/run"
    "/sbin"
    "/sys"
    "/usr"
    "/var/run"
  ];

  sensitiveName =
    name:
    let
      upper = lib.toUpper name;
      matchesToken =
        token:
        upper == token
        || lib.hasPrefix "${token}_" upper
        || lib.hasSuffix "_${token}" upper
        || lib.hasInfix "_${token}_" upper;
    in
    lib.any matchesToken [
      "APIKEY"
      "AUTH"
      "AUTHORIZATION"
      "BEARER"
      "CREDENTIAL"
      "KEY"
      "PASSWORD"
      "PRIVATEKEY"
      "SECRET"
      "TOKEN"
    ];

  containsSensitiveValue =
    value:
    if builtins.isAttrs value then
      lib.any (name: sensitiveName name || containsSensitiveValue value.${name}) (lib.attrNames value)
    else if builtins.isList value then
      lib.any containsSensitiveValue value
    else
      false;

  forbiddenOptionPrefixes = [
    "--add-host"
    "--cap-add"
    "--cgroup-parent"
    "--cgroupns"
    "--connection"
    "--context"
    "--cpus"
    "--device"
    "--device-cgroup-rule"
    "--device-read-bps"
    "--device-read-iops"
    "--device-write-bps"
    "--device-write-iops"
    "--dns"
    "--dns-option"
    "--dns-search"
    "--env"
    "--env-file"
    "--expose"
    "--group-add"
    "--gpus"
    "--ipc"
    "--link"
    "--log-driver"
    "--mac-address"
    "--memory"
    "--memory-swap"
    "--mount"
    "--net"
    "--network"
    "--network-alias"
    "--oom-kill-disable"
    "--pid"
    "--pids-limit"
    "--privileged"
    "--publish"
    "--publish-all"
    "--read-only"
    "--rootfs"
    "--runtime"
    "--security-opt"
    "--sysctl"
    "--tmpfs"
    "--ulimit"
    "--url"
    "--user"
    "--userns"
    "--uts"
    "--volume"
    "--volumes-from"
  ];
  hasForbiddenOption =
    option:
    lib.any (prefix: option == prefix || lib.hasPrefix "${prefix}=" option) forbiddenOptionPrefixes
    || lib.any (prefix: lib.hasPrefix prefix option) [
      "-e"
      "-H"
      "-P"
      "-h"
      "-p"
      "-u"
      "-v"
    ];

  volumeIsSafe =
    allowedWritableSources: allowedWritableDestinations: approvedReadOnlySources: approvedReadOnlyDestinations: volume:
    let
      parts = volumeParts volume;
      source = if parts == [ ] then "" else volumeSource volume;
      destination = if lib.length parts < 2 then "" else volumeDestination volume;
      sourceAllowedRw = lib.elem source allowedWritableSources;
      destinationAllowedRw = lib.elem destination allowedWritableDestinations;
      sourceIsApprovedRo = pathWithin "/nix/store" source || lib.elem source approvedReadOnlySources;
      sourceSyntaxSafe = canonicalPath source;
      destinationSyntaxSafe = canonicalPath destination;
      sensitiveSource = forbiddenWritableSource source;
      sensitiveDestination = lib.any (root: pathWithin root destination) sensitiveDestinations;
      readOnly = volumeIsReadOnly volume;
    in
    volumeSyntaxIsSafe volume
    && sourceSyntaxSafe
    && destinationSyntaxSafe
    && !sensitiveSource
    && (sourceAllowedRw || (sourceIsApprovedRo && readOnly))
    && (!sensitiveDestination || (readOnly && lib.elem destination approvedReadOnlyDestinations))
    && (readOnly || (sourceAllowedRw && destinationAllowedRw));
in
{
  inherit
    isSecure
    containsSensitiveValue
    canonicalPath
    forbiddenWritableSource
    statePathIsSafe
    ;

  apply =
    {
      profile,
      backend,
      name,
      owner,
      baseConfig,
      overrides ? { },
      allowedWritableSources,
      allowedWritableDestinations,
      pidsLimit,
      resources,
      brokerNetwork ? null,
      approvedEnvironmentFiles ? [ ],
      approvedReadOnlySources ? [ ],
      approvedReadOnlyDestinations ? [ ],
      automaticStart ? false,
      brokerPolicyEnabled ? false,
      workerEnabled ? false,
      workspaceQuotaEnabled ? false,
    }:
    let
      secure = isSecure profile;
      merged = lib.recursiveUpdate baseConfig overrides;
      extraOptions = merged.extraOptions or [ ];
      preRunExtraOptions = merged.preRunExtraOptions or [ ];
      volumes = merged.volumes or [ ];
      capabilities = merged.capabilities or { };
      labels = merged.labels or { };
      expectedNetworks = lib.optional (brokerNetwork != null) brokerNetwork;
      nonRootOwner = lib.match "^[1-9][0-9]*:[1-9][0-9]*$" owner != null;
      secureExtraOptions = [
        "--read-only"
        "--security-opt=no-new-privileges:true"
        "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=${resources.tmpfsSize}"
        "--tmpfs=/run:rw,nosuid,nodev,noexec,size=${resources.runTmpfsSize}"
        "--tmpfs=/var/tmp:rw,nosuid,nodev,noexec,size=${resources.tmpfsSize}"
        "--memory=${resources.memory}"
        "--memory-swap=${resources.memorySwap}"
        "--cpus=${resources.cpus}"
        "--pids-limit=${toString pidsLimit}"
        "--ulimit=nofile=${toString resources.nofile}:${toString resources.nofile}"
        "--ulimit=nproc=${toString pidsLimit}:${toString pidsLimit}"
      ]
      ++ lib.optional (backend == "docker") "--security-opt=apparmor=docker-default"
      ++ lib.optional (brokerNetwork == null) "--network=none"
      ++ lib.optionals (brokerNetwork != null) [
        "--dns=127.0.0.1"
        "--dns-option=attempts:1"
        "--sysctl=net.ipv6.conf.all.disable_ipv6=1"
      ]
      ++ lib.optional (backend == "docker") "--runtime=runsc";
      securePreRunOptions = lib.optionals (backend == "podman") [
        "--runtime"
        "runsc"
      ];
    in
    {
      assertions = [
        {
          assertion = lib.match "^[a-z0-9][a-z0-9-]*$" name != null;
          message = "tentaflake: agent name ${name} must contain only lowercase ASCII letters, digits, and hyphens.";
        }
      ]
      ++ lib.optionals secure [
        {
          assertion = !(merged.privileged or false);
          message = "tentaflake: secure agent ${name} may not be privileged.";
        }
        {
          assertion = !automaticStart || brokerPolicyEnabled;
          message = "tentaflake: automatically started secure agent ${name} requires an enabled broker policy; keep it stopped while defining the policy.";
        }
        {
          assertion = !automaticStart || workerEnabled;
          message = "tentaflake: automatically started secure agent ${name} requires an enabled disposable worker; keep it stopped while wiring runtime tools to the worker queue.";
        }
        {
          assertion = !automaticStart || workspaceQuotaEnabled;
          message = "tentaflake: automatically started secure agent ${name} requires an enabled fixed-size workspace quota; keep it stopped until the quota migration is complete.";
        }
        {
          assertion = (merged.user or owner) == owner;
          message = "tentaflake: secure agent ${name} must run as explicit user ${owner}.";
        }
        {
          assertion = nonRootOwner;
          message = "tentaflake: secure agent ${name} requires explicit non-root numeric UID and GID.";
        }
        {
          assertion = (merged.ports or [ ]) == [ ];
          message = "tentaflake: secure agent ${name} may not publish container ports.";
        }
        {
          assertion = (merged.networks or [ ]) == expectedNetworks;
          message = "tentaflake: secure agent ${name} may join only its declared isolated broker network.";
        }
        {
          assertion = (merged.devices or [ ]) == [ ];
          message = "tentaflake: secure agent ${name} may not receive host devices.";
        }
        {
          assertion = !(lib.any (value: value == true) (lib.attrValues capabilities));
          message = "tentaflake: secure agent ${name} may not add Linux capabilities.";
        }
        {
          assertion =
            (merged.environmentFiles or [ ]) == approvedEnvironmentFiles
            && lib.all (
              path: lib.match "^/run/tentaflake-broker/[A-Za-z0-9._+/-]+[.]env$" path != null
            ) approvedEnvironmentFiles;
          message = "tentaflake: secure agent ${name} may receive only its runtime-generated virtual broker credential environment file.";
        }
        {
          assertion = (merged.imageFile or null) == null && (merged.imageStream or null) == null;
          message = "tentaflake: secure agent ${name} may not replace its reviewed registry digest with a caller-supplied image archive or stream.";
        }
        {
          assertion = !(containsSensitiveValue (merged.environment or { }));
          message = "tentaflake: secure agent ${name} has a secret-like environment key, which would enter the Nix store.";
        }
        {
          assertion = !(lib.any hasForbiddenOption (extraOptions ++ preRunExtraOptions));
          message = "tentaflake: secure agent ${name} extra options attempt to override a security invariant.";
        }
        {
          assertion = lib.all (volumeIsSafe allowedWritableSources allowedWritableDestinations
            approvedReadOnlySources
            approvedReadOnlyDestinations
          ) volumes;
          message = "tentaflake: secure agent ${name} has an unsupported, writable, relative, or sensitive bind mount outside its declared state/workspace boundary.";
        }
        {
          assertion = pidsLimit != null && pidsLimit > 0;
          message = "tentaflake: secure agent ${name} requires a positive PID limit.";
        }
      ];

      container =
        if !secure then
          merged
          // {
            labels = labels // {
              "io.tentaflake.agent" = "true";
              "io.tentaflake.security-profile" = profile;
            };
          }
        else
          merged
          // {
            user = owner;
            privileged = false;
            capabilities = capabilities // {
              ALL = false;
            };
            ports = [ ];
            networks = expectedNetworks;
            environmentFiles = approvedEnvironmentFiles;
            devices = [ ];
            labels = labels // {
              "io.tentaflake.agent" = "true";
              "io.tentaflake.security-profile" = profile;
              "io.tentaflake.brokered-egress" = if brokerNetwork == null then "false" else "true";
            };
            preRunExtraOptions = preRunExtraOptions ++ securePreRunOptions;
            extraOptions = extraOptions ++ secureExtraOptions;
          };
    };
}
