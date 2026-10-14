{ lib }:
let
  limits = import ../lib/resourceLimits.nix { inherit lib; };
  contract = import ../lib/runtimeContract.nix { inherit lib; };
  definition = {
    schemaVersion = 1;
    image = "example.invalid/agent@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
    command = [ "/app/agent" ];
  };
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
  invalidSizes = [
    null
    true
    1
    1.5
    [ ]
    { }
    ""
    "0"
    "01"
    "-1"
    "1.0g"
    "1e3"
    "1g "
    "1 g"
    "1p"
    "999999999999999999999999999999999999"
    "9223372036854775808"
    "1099511627777"
    "1025g"
    "2t"
  ];
  invalidCpus = [
    null
    true
    1
    1.5
    [ ]
    { }
    ""
    "0"
    "0.00000"
    "01"
    "00.5"
    ".5"
    "1."
    "-1"
    "1e3"
    "1 "
    "0.000001"
    "1.000000"
    "1024.00001"
    "1025"
    "999999999999999999999999999999999999"
  ];
  resourceOptions = [
    "--memory=512m"
    "--memory-swap=1g"
    "--cpus=0.5"
    "--pids-limit=64"
    "--ulimit=nofile=128:128"
    "--ulimit=nproc=64:64"
  ];
  tmpfsOptions = [
    "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=32m"
    "--tmpfs=/run:rw,nosuid,nodev,noexec,size=16m"
    "--tmpfs=/var/tmp:rw,nosuid,nodev,noexec,size=32m"
  ];
  replace =
    from: to: options:
    map (option: if option == from then to else option) options;
  brokenResources = [
    null
    [ null ]
    (resourceOptions ++ [ "--memory=256m" ])
    (
      resourceOptions
      ++ [
        "--memory"
        "256m"
      ]
    )
    (resourceOptions ++ [ "-m256m" ])
    (
      resourceOptions
      ++ [
        "-m"
        "256m"
      ]
    )
    (resourceOptions ++ [ "--cpus=0.25" ])
    (resourceOptions ++ [ "--pids-limit=32" ])
    (resourceOptions ++ [ "--ulimit=nofile=128:128" ])
    (
      resourceOptions
      ++ [
        "--ulimit"
        "nproc=64:64"
      ]
    )
    (replace "--memory=512m" "--memory=2t" resourceOptions)
    (replace "--memory-swap=1g" "--memory-swap=256m" resourceOptions)
    (replace "--cpus=0.5" "--cpus=0.000001" resourceOptions)
    (replace "--pids-limit=64" "--pids-limit=9223372036854775808" resourceOptions)
    (replace "--ulimit=nofile=128:128" "--ulimit=nofile=128:256" resourceOptions)
    (replace "--ulimit=nofile=128:128" "--ulimit=nofile=4294967295:4294967295" resourceOptions)
    (replace "--ulimit=nproc=64:64" "--ulimit=nproc=32:32" resourceOptions)
  ]
  ++ map (missing: lib.remove missing resourceOptions) resourceOptions;
  brokenTmpfs = [
    null
    [ null ]
    (tmpfsOptions ++ [ (lib.head tmpfsOptions) ])
    (
      tmpfsOptions
      ++ [
        "--tmpfs"
        "/tmp:rw,nosuid,nodev,noexec,size=32m"
      ]
    )
  ]
  ++ map (missing: lib.remove missing tmpfsOptions) tmpfsOptions
  ++ map (bad: [ bad ] ++ lib.tail tmpfsOptions) [
    "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=65g"
    "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=0"
    "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=32m,size=64m"
    "--tmpfs=/tmp:rw,nosuid,nodev,exec,size=32m"
    "--tmpfs=/tmp:rw,nosuid,nodev,size=32m"
    "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=32m:extra"
    "--tmpfs=/run:rw,nosuid,nodev,noexec,size=32m"
    "--tmpfs=/other:rw,nosuid,nodev,noexec,size=32m"
  ];
in
assert limits.sizeBytes limits.maxMemoryBytes "1T" == limits.maxMemoryBytes;
assert limits.sizeBytes limits.maxTmpfsBytes "64G" == limits.maxTmpfsBytes;
assert limits.sizeBytes limits.maxMemoryBytes "1099511627776" == limits.maxMemoryBytes;
assert limits.sizeBytes limits.maxMemoryBytes "1k" == 1024;
assert limits.sizeBytes limits.maxMemoryBytes "1" == 1;
assert lib.all (
  value:
  !limits.validSize limits.maxMemoryBytes value && limits.sizeBytes limits.maxMemoryBytes value == 0
) invalidSizes;
assert !limits.validSize limits.maxTmpfsBytes "68719476737";
assert !limits.validSize limits.maxTmpfsBytes "65g";
assert limits.cpuNano "0.00001" == 10000;
assert limits.cpuNano "1.00100" == 1001000000;
assert limits.cpuNano "1024.00000" == 1024000000000;
assert lib.all (value: !limits.validCpus value && limits.cpuNano value == 0) invalidCpus;
assert limits.validResource "nofile" 4294967294;
assert limits.validResource "pidsLimit" 1;
assert lib.all
  (value: !limits.validResource "nofile" value && !limits.validResource "pidsLimit" value)
  [
    null
    true
    "1"
    1.0
    0
    (-1)
    4294967295
  ];
assert !limits.validResource "unknown" 1;
assert (contract (definition // { resources.memory = "1t"; })).resources.memory == "1t";
assert lib.all (resources: rejects (contract (definition // { inherit resources; }))) (
  [
    { memory = "2t"; }
    { tmpfsSize = "65g"; }
    { runTmpfsSize = "65g"; }
    { nofile = 4294967295; }
    { pidsLimit = 1.0; }
    {
      memory = "512m";
      memorySwap = "256m";
    }
  ]
  ++ map (memory: { inherit memory; }) invalidSizes
  ++ map (cpus: { inherit cpus; }) invalidCpus
);
assert
  limits.ociResources resourceOptions == {
    memoryBytes = 512 * 1024 * 1024;
    memorySwapBytes = 1024 * 1024 * 1024;
    nanoCpus = 500000000;
    pidsLimit = 64;
    nofile = 128;
    nproc = 64;
  };
assert
  limits.ociTmpfs tmpfsOptions == {
    "/run" = 16 * 1024 * 1024;
    "/tmp" = 32 * 1024 * 1024;
    "/var/tmp" = 32 * 1024 * 1024;
  };
assert lib.all (options: limits.ociResources options == null) brokenResources;
assert lib.all (options: limits.ociTmpfs options == null) brokenTmpfs;
true
