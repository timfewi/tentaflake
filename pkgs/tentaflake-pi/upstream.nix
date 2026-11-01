# Build only the actual Durable SDK and its workspace dependencies. Every
# external version/integrity comes from the reviewed upstream lock unchanged.
{ pkgs, source }:
let
  inherit (pkgs) lib;
  sourceRevision = "35dfa757378bd7a8570670e99c2687bdd1039a06";
  original = builtins.fromJSON (builtins.readFile (source + "/package.json"));
  originalLock = builtins.fromJSON (builtins.readFile (source + "/package-lock.json"));
  # Git deliberately omits generated provider JSON. Supply only that data from
  # the versioned MIT npm artifact; the pinned source's unchanged offline check
  # validates the full provider/model/API structure, manifest and file hashes.
  modelDataPackage = pkgs.fetchurl {
    url = "https://registry.npmjs.org/@earendil-works/pi-ai/-/pi-ai-1.4.0.tgz";
    hash = "sha256-tA4rqLtvwzKQj4uW6DfcLR/ftF65zTgvWu0RFGKYHjw=";
  };
  selected = [
    "telemetry"
    "chord"
    "codemode"
    "ai"
    "durable"
  ];
  workspacePaths = map (name: "packages/${name}") selected;
  manifests = lib.genAttrs selected (
    name:
    builtins.removeAttrs (builtins.fromJSON (
      builtins.readFile (source + "/packages/${name}/package.json")
    )) [ "devDependencies" ]
  );
  buildDependencies = lib.getAttrs [
    "typescript"
    "shx"
    "@types/node"
    "esbuild"
  ] original.devDependencies;
  package = {
    name = "tentaflake-pi-sdk";
    version = "1.4.0";
    private = true;
    type = "module";
    workspaces = workspacePaths;
    devDependencies = buildDependencies;
    inherit (original) engines overrides;
  };
  entries = originalLock.packages;
  resolve =
    from: name:
    let
      parts = if from == "" then [ ] else lib.splitString "/" from;
      candidates = builtins.genList (
        index:
        let
          prefix = lib.concatStringsSep "/" (lib.take (builtins.length parts - index) parts);
        in
        lib.optionalString (prefix != "") "${prefix}/" + "node_modules/${name}"
      ) (builtins.length parts + 1);
      found = lib.findFirst (path: builtins.hasAttr path entries) null candidates;
    in
    found;
  workspaceLinks = map (name: "node_modules/${manifests.${name}.name}") selected;
  roots =
    workspacePaths
    ++ workspaceLinks
    ++ map (name: "node_modules/${name}") (builtins.attrNames buildDependencies);
  closure = builtins.genericClosure {
    startSet = map (key: { inherit key; }) roots;
    operator =
      node:
      let
        entry = entries.${node.key};
        requiredPeers = lib.filterAttrs (name: _: !(entry.peerDependenciesMeta.${name}.optional or false)) (
          entry.peerDependencies or { }
        );
        required = (entry.dependencies or { }) // requiredPeers;
        optional = entry.optionalDependencies or { };
        dependencies = required // optional;
      in
      if entry.link or false then
        assert builtins.elem entry.resolved workspacePaths;
        [ { key = entry.resolved; } ]
      else
        lib.concatMap (
          name:
          let
            path = resolve node.key name;
          in
          if path != null then
            [ { key = path; } ]
          else if builtins.hasAttr name optional then
            [ ]
          else
            throw "Pi SDK lock is missing a required dependency; refusing to resolve a new version."
        ) (builtins.attrNames dependencies);
  };
  retained = lib.genAttrs (map (node: node.key) closure) (
    path:
    if builtins.elem path workspacePaths then
      builtins.removeAttrs entries.${path} [ "devDependencies" ]
    else
      entries.${path}
  );
  packageLock = originalLock // {
    inherit (package) name version;
    packages = retained // {
      "" = package;
    };
  };
  manifestFiles = lib.genAttrs selected (
    name: pkgs.writeText "pi-${name}-package.json" (builtins.toJSON manifests.${name})
  );
  preparedSource =
    pkgs.runCommand "tentaflake-pi-sdk-source"
      {
        nativeBuildInputs = [
          pkgs.gnutar
          pkgs.gzip
        ];
      }
      ''
        mkdir -p "$out/packages"
        cp ${pkgs.writeText "pi-sdk-package.json" (builtins.toJSON package)} "$out/package.json"
        cp ${pkgs.writeText "pi-sdk-package-lock.json" (builtins.toJSON packageLock)} "$out/package-lock.json"
        cp ${source}/tsconfig.base.json "$out/tsconfig.base.json"
        cp ${source}/LICENSE "$out/LICENSE"
        ${lib.concatMapStringsSep "\n" (name: ''
          cp -r ${source}/packages/${name} "$out/packages/${name}"
          chmod -R u+w "$out/packages/${name}"
          cp ${manifestFiles.${name}} "$out/packages/${name}/package.json"
        '') selected}
        mkdir -p "$out/packages/ai/src/providers/data"
        tar --extract --gzip --no-same-owner --file ${modelDataPackage} \
          --strip-components=4 --directory "$out/packages/ai/src/providers/data" \
          package/dist/providers/data
      '';
  runtime = pkgs.buildNpmPackage {
    pname = "tentaflake-pi-sdk";
    version = "1.4.0";
    src = preparedSource;
    nodejs = pkgs.nodejs_24;
    npmDeps = pkgs.importNpmLock {
      npmRoot = source;
      inherit package packageLock;
    };
    npmConfigHook = pkgs.importNpmLock.npmConfigHook;
    npmFlags = [
      "--ignore-scripts"
      "--no-audit"
      "--no-fund"
    ];
    buildPhase = ''
      runHook preBuild
      # importNpmLock may materialize local workspaces. Always bind imports to
      # these reviewed workspace sources before compiling their dist exports.
      ${lib.concatMapStringsSep "\n" (name: ''
        rm -rf "node_modules/${manifests.${name}.name}"
        mkdir -p "node_modules/@earendil-works"
        ln -s "../../packages/${name}" "node_modules/${manifests.${name}.name}"
      '') selected}
      npm --workspace @earendil-works/pi-telemetry run build --ignore-scripts
      npm --workspace @earendil-works/chord run build --ignore-scripts
      npm --workspace @earendil-works/pi-codemode run build --ignore-scripts
      npm --workspace @earendil-works/pi-ai run build:offline --ignore-scripts
      npm --workspace @earendil-works/pi-durable run build --ignore-scripts
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p "$out/lib/pi/packages" "$out/share/doc/tentaflake-pi-sdk"
      cp -a node_modules "$out/lib/pi/node_modules"
      ${lib.concatMapStringsSep "\n" (name: ''
        cp -r packages/${name} "$out/lib/pi/packages/${name}"
      '') selected}
      cp LICENSE "$out/share/doc/tentaflake-pi-sdk/LICENSE"
      runHook postInstall
    '';
    doInstallCheck = true;
    installCheckPhase = ''
      runHook preInstallCheck
      cd "$out/lib/pi"
      node --input-type=module -e 'await import("@earendil-works/pi-durable"); await import("@earendil-works/pi-durable/storage/sqlite/node");'
      runHook postInstallCheck
    '';
    meta = {
      description = "Pinned Pi Durable SDK compiled without provider/model requests";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
    };
  };
in
assert (source.rev or null) == sourceRevision;
assert originalLock.lockfileVersion == 3;
{
  inherit sourceRevision runtime;
  nodeModules = "${runtime}/lib/pi/node_modules";
}
