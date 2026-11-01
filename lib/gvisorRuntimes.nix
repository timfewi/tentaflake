{ lib, pkgs }:
let
  researchRuntime = pkgs.writeShellScriptBin "runsc-research" ''
    exec ${lib.getExe' pkgs.gvisor "runsc"} --host-uds=open "$@"
  '';
  piRuntime = pkgs.writeShellScriptBin "runsc-pi" ''
    exec ${lib.getExe' pkgs.gvisor "runsc"} --host-uds=all "$@"
  '';
  stock = {
    name = "runsc";
    path = lib.getExe' pkgs.gvisor "runsc";
  };
  research = {
    name = "runsc-research";
    runtime = researchRuntime;
    path = lib.getExe researchRuntime;
  };
  pi = {
    name = "runsc-pi";
    runtime = piRuntime;
    path = lib.getExe piRuntime;
  };
  at =
    config: keys: fallback:
    lib.attrByPath keys fallback config;
  normalize =
    value:
    if !lib.hasPrefix "/" value then
      value
    else
      "/"
      + lib.concatStringsSep "/" (
        lib.foldl' (
          parts: part:
          if part == "" || part == "." then
            parts
          else if part == ".." then
            if parts == [ ] then [ ] else lib.init parts
          else
            parts ++ [ part ]
        ) [ ] (lib.splitString "/" value)
      );
  anyString =
    predicate: value:
    if builtins.isString value then
      predicate value
    else if builtins.isList value then
      lib.any (anyString predicate) value
    else if builtins.isAttrs value then
      lib.any (anyString predicate) (builtins.attrValues value)
    else
      false;
  references =
    runtime:
    anyString (
      value:
      normalize value == runtime.path
      || lib.hasInfix (builtins.unsafeDiscardStringContext "${runtime.runtime}/") value
    );
  hostUdsArguments = anyString (lib.hasInfix "--host-uds");
  elevatedRegistration =
    value:
    references research value
    || references pi value
    || (anyString (value: normalize value == stock.path) value && hostUdsArguments value);
  backend = config: at config [ "virtualisation" "oci-containers" "backend" ] "";
  registrations =
    config: selectedBackend:
    if selectedBackend == "docker" then
      at config [ "virtualisation" "docker" "daemon" "settings" "runtimes" ] { }
    else
      at config [ "virtualisation" "containers" "containersConf" "settings" "engine" "runtimes" ] { };
  defaultRuntime =
    config: selectedBackend:
    if selectedBackend == "docker" then
      at config [ "virtualisation" "docker" "daemon" "settings" "default-runtime" ] "runc"
    else
      at config [ "virtualisation" "containers" "containersConf" "settings" "engine" "runtime" ] "crun";
  expected =
    selectedBackend: runtime:
    if selectedBackend == "docker" then { inherit (runtime) path; } else [ runtime.path ];
  selectorElevatedIn =
    config: selectedBackend: selector:
    builtins.elem selector [
      research.name
      pi.name
    ]
    || elevatedRegistration selector
    || elevatedRegistration ((registrations config selectedBackend).${selector} or { });
in
rec {
  inherit stock research pi;
  selectorElevated = config: selector: selectorElevatedIn config (backend config) selector;
  stockSupported =
    config:
    builtins.elem (backend config) [
      "docker"
      "podman"
    ]
    && ((registrations config (backend config)).runsc or null) == expected (backend config) stock
    && (backend config != "docker" || at config [ "virtualisation" "docker" "extraOptions" ] "" == "");
  aliasesSupported =
    config:
    let
      researchEnabled = at config [ "tentaflake" "research" "agents" ] { } != { };
      piEnabled =
        at config [ "tentaflake" "piRuntime" "enable" ] false
        && at config [ "tentaflake" "piRuntime" "transport" ] null == "runsc-pi-v1";
    in
    lib.all
      (
        selectedBackend:
        lib.all (
          name:
          let
            value = (registrations config selectedBackend).${name};
          in
          if name == research.name then
            researchEnabled && value == expected selectedBackend research
          else if name == pi.name then
            piEnabled && value == expected selectedBackend pi
          else
            !elevatedRegistration value
        ) (builtins.attrNames (registrations config selectedBackend))
        && !selectorElevatedIn config selectedBackend (defaultRuntime config selectedBackend)
      )
      [
        "docker"
        "podman"
      ];
  configurationSupported = config: stockSupported config && aliasesSupported config;
  # Read both OCI flag forms, including direct executable selectors. Unknown
  # independent consumer runtimes remain host policy, not agent admission.
  selectors =
    container:
    let
      scan =
        options:
        if options == [ ] then
          [ ]
        else
          let
            first = lib.head options;
            rest = lib.tail options;
          in
          if first == "--runtime" then
            if rest == [ ] then [ "" ] else [ (lib.head rest) ] ++ scan (lib.tail rest)
          else if lib.hasPrefix "--runtime=" first then
            [ (lib.removePrefix "--runtime=" first) ] ++ scan rest
          else
            scan rest;
    in
    scan ((container.extraOptions or [ ]) ++ (container.preRunExtraOptions or [ ]));
}
