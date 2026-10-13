{ lib }:
let
  maxMemoryBytes = 1024 * 1024 * 1024 * 1024;
  maxTmpfsBytes = 64 * 1024 * 1024 * 1024;
  maxCpus = 1024;
  maxInteger = 4294967294;
  positive = value: builtins.isInt value && value > 0 && value <= maxInteger;
  sizeComponents =
    value:
    let
      matched =
        if builtins.isString value then builtins.match "^([1-9][0-9]*)([kKmMgGtT]?)$" value else null;
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
  validSize =
    maximum: value:
    let
      components = sizeComponents value;
    in
    components != null && components.amount <= maximum / components.factor;
  sizeBytes =
    maximum: value:
    let
      components = sizeComponents value;
    in
    if validSize maximum value then components.amount * components.factor else 0;
  digitValues = {
    "0" = 0;
    "1" = 1;
    "2" = 2;
    "3" = 3;
    "4" = 4;
    "5" = 5;
    "6" = 6;
    "7" = 7;
    "8" = 8;
    "9" = 9;
  };
  digitsToInt =
    value:
    lib.foldl' (total: digit: total * 10 + digitValues.${digit}) 0 (lib.stringToCharacters value);
  cpuValue =
    value:
    let
      matched =
        if builtins.isString value && builtins.stringLength value <= 16 then
          builtins.match "^([1-9][0-9]*|0)([.]([0-9]+))?$" value
        else
          null;
      fraction = if matched == null || lib.elemAt matched 2 == null then "" else lib.elemAt matched 2;
      fractionLength = builtins.stringLength fraction;
      fractionFactor = lib.foldl' (total: _: total * 10) 1 (lib.replicate (9 - fractionLength) null);
    in
    if matched == null || builtins.stringLength (lib.head matched) > 4 || fractionLength > 5 then
      0
    else
      digitsToInt (lib.head matched) * 1000000000 + digitsToInt fraction * fractionFactor;
  validCpus =
    value:
    let
      nano = cpuValue value;
    in
    nano > 0 && nano <= maxCpus * 1000000000;
  cpuNano = value: if validCpus value then cpuValue value else 0;
  validResource =
    key: value:
    if
      lib.elem key [
        "nofile"
        "pidsLimit"
      ]
    then
      positive value
    else if key == "cpus" then
      validCpus value
    else if
      lib.elem key [
        "memory"
        "memorySwap"
      ]
    then
      validSize maxMemoryBytes value
    else if
      lib.elem key [
        "tmpfsSize"
        "runTmpfsSize"
      ]
    then
      validSize maxTmpfsBytes value
    else
      false;

  # Parse only the explicit OCI forms emitted by containerSecurity. Ambiguous
  # or incomplete declarations cannot be evidence of live resource limits.
  optionValues =
    name: options:
    map (option: if lib.hasPrefix "${name}=" option then lib.removePrefix "${name}=" option else null) (
      lib.filter (
        option: option == name || lib.hasPrefix "${name}=" option || lib.hasPrefix "${name} " option
      ) options
    );
  singleValue =
    name: options:
    let
      values = optionValues name options;
    in
    if builtins.length values == 1 then lib.head values else null;
  optionInt =
    value:
    let
      parsed =
        if
          builtins.isString value
          && builtins.stringLength value <= 10
          && builtins.match "^[1-9][0-9]*$" value != null
        then
          builtins.fromJSON value
        else
          0;
    in
    if positive parsed then parsed else 0;
  validOptions = options: builtins.isList options && lib.all builtins.isString options;
  ulimit =
    value:
    let
      matched =
        if builtins.isString value then
          builtins.match "^(nofile|nproc)=([1-9][0-9]*):([1-9][0-9]*)$" value
        else
          null;
      soft = if matched == null then 0 else optionInt (lib.elemAt matched 1);
      hard = if matched == null then 0 else optionInt (lib.elemAt matched 2);
    in
    if matched == null || soft == 0 || soft != hard then
      null
    else
      {
        name = lib.head matched;
        value = soft;
      };
  ociResources =
    options:
    let
      memoryBytes = sizeBytes maxMemoryBytes (singleValue "--memory" options);
      memorySwapBytes = sizeBytes maxMemoryBytes (singleValue "--memory-swap" options);
      nanoCpus = cpuNano (singleValue "--cpus" options);
      pidsLimit = optionInt (singleValue "--pids-limit" options);
      ulimits = map ulimit (optionValues "--ulimit" options);
      ulimitNames = map (value: value.name) ulimits;
      limits = builtins.listToAttrs ulimits;
    in
    if
      !validOptions options
      || lib.any (lib.hasPrefix "-m") options
      || memoryBytes == 0
      || memorySwapBytes < memoryBytes
      || nanoCpus == 0
      || pidsLimit == 0
      || lib.elem null ulimits
      ||
        lib.sort builtins.lessThan ulimitNames != [
          "nofile"
          "nproc"
        ]
      || limits.nproc != pidsLimit
    then
      null
    else
      {
        inherit
          memoryBytes
          memorySwapBytes
          nanoCpus
          pidsLimit
          ;
        inherit (limits) nofile nproc;
      };
  tmpfsMount =
    value:
    let
      parts = if builtins.isString value then lib.splitString ":" value else [ ];
      options = if builtins.length parts == 2 then lib.splitString "," (lib.elemAt parts 1) else [ ];
      sizes = lib.filter (lib.hasPrefix "size=") options;
      bytes =
        if builtins.length sizes == 1 then
          sizeBytes maxTmpfsBytes (lib.removePrefix "size=" (lib.head sizes))
        else
          0;
    in
    if
      builtins.length parts != 2
      || builtins.length options != 5
      || !lib.all (option: lib.elem option options) [
        "rw"
        "nosuid"
        "nodev"
        "noexec"
      ]
      || bytes == 0
    then
      null
    else
      {
        name = lib.head parts;
        value = bytes;
      };
  ociTmpfs =
    options:
    let
      mounts = map tmpfsMount (optionValues "--tmpfs" options);
      destinations = map (mount: mount.name) mounts;
    in
    if
      !validOptions options
      || lib.elem null mounts
      ||
        lib.sort builtins.lessThan destinations != [
          "/run"
          "/tmp"
          "/var/tmp"
        ]
    then
      null
    else
      builtins.listToAttrs mounts;
in
{
  inherit
    maxMemoryBytes
    maxTmpfsBytes
    validSize
    sizeBytes
    validCpus
    cpuNano
    validResource
    ociResources
    ociTmpfs
    ;
}
