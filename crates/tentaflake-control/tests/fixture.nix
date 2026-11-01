# Source-only synthetic fixture. Building this does not evaluate/build/activate a host.
{ pkgs }:
pkgs.runCommand "tentaflake-control-synthetic-fixture" { } ''
  mkdir -p "$out/template" "$out/linked-template"
  cat > "$out/template/flake.nix" <<'EOF'
  { outputs = { self }: { }; }
  EOF
  cat > "$out/template/flake.lock" <<'EOF'
  {"nodes":{"root":{}},"root":"root","version":7}
  EOF
  printf 'schemaVersion = 1\nagents = []\n' > "$out/template/tentaflake.toml"
  cp "$out/template/"* "$out/linked-template/"
  ln -s "$out/template/flake.nix" "$out/linked-template/escape.nix"
  cp ${../../../schema/system-v1.json} "$out/schema.json"
  cat > "$out/normalizer" <<'EOF'
  #!${pkgs.python3}/bin/python3
  import json, sys, tomllib
  if len(sys.argv) != 5 or sys.argv[1:3] != ["config", "normalize"] or sys.argv[4] != "--json":
      sys.exit(2)
  with open(sys.argv[3], "rb") as source:
      value = tomllib.load(source)
  if value.get("schemaVersion") != 1 or value.get("agents") != [] or set(value) - {"schemaVersion", "agents", "sandbox"}:
      sys.exit(2)
  if "sandbox" in value and (set(value["sandbox"]) != {"profile"} or value["sandbox"]["profile"] not in ["balanced", "strict", "dev"]):
      sys.exit(2)
  print(json.dumps(value, sort_keys=True, separators=(",", ":")))
  EOF
  cat > "$out/builder" <<'EOF'
  #!${pkgs.runtimeShell}
  [ "$#" -eq 5 ] && [ "$1" = build ] && [ "$2" = --no-link ] && [ "$3" = --json ] && [ "$4" = --no-write-lock-file ] || exit 90
  case "$5" in
    /*/b_*\#nixosConfigurations.ok.config.system.build.toplevel)
      printf '%s' '[{"outputs":{"out":"/nix/store/00000000000000000000000000000000-synthetic-system"}}]';;
    /*/b_*\#nixosConfigurations.fail.config.system.build.toplevel)
      printf 'synthetic-private-error' >&2; exit 1;;
    /*/b_*\#nixosConfigurations.badpath.config.system.build.toplevel)
      printf '%s' '[{"outputs":{"out":"/etc/passwd"}}]';;
    /*/b_*\#nixosConfigurations.noisy.config.system.build.toplevel)
      while :; do printf '0123456789012345678901234567890123456789'; done;;
    /*/b_*\#nixosConfigurations.slow.config.system.build.toplevel)
      while :; do :; done;;
    *) exit 91;;
  esac
  EOF
  chmod 555 "$out/normalizer" "$out/builder"
''
