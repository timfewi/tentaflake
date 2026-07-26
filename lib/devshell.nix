# ────────────────────────────────────────────────────────────
# devshell.nix — the contributor environment behind `nix develop`
#
# Same visual language as the login banner (modules/shell.nix): braille logo in
# cyan on the left, a dim key/value column on the right. The facts differ —
# a contributor cares about the checkout and the gates, not fleet health.
# ────────────────────────────────────────────────────────────

{ pkgs }:
let
  inherit (pkgs) lib;

  # Single source of truth for the art is public/tentaflake-shell-logo.txt —
  # the same file the login banner reads. Indented at build time so the banner
  # script stays a plain render loop.
  logo = lib.concatMapStringsSep "\n" (line: "  " + line) (
    lib.splitString "\n" (lib.removeSuffix "\n" (builtins.readFile ../public/tentaflake-shell-logo.txt))
  );

  # The cheat sheet mirrors the justfile — the five recipes worth knowing on
  # entry. `just` itself lists the rest, so this never has to grow.
  commands = [
    {
      cmd = "just";
      what = "list every recipe";
    }
    {
      cmd = "just ci";
      what = "full local gate (CI + both ISOs)";
    }
    {
      cmd = "just check";
      what = "nix flake check";
    }
    {
      cmd = "just fmt";
      what = "format the tree with nixfmt";
    }
    {
      cmd = "just banner";
      what = "preview the login banner";
    }
  ];

  # writeShellApplication shellchecks at build time, so building the dev shell
  # lints this script — same guarantee `just shellcheck` gives scripts/*.sh.
  banner = pkgs.writeShellApplication {
    name = "tentaflake-devshell-banner";
    runtimeInputs = [ pkgs.coreutils ];
    text = ''
      # Silent for `nix develop --command …` (CI pipes it) and for opt-outs.
      if [ ! -t 1 ] || [ -n "''${TENTAFLAKE_NO_BANNER:-}" ]; then
        exit 0
      fi

      bold=$(printf '\033[1m'); dim=$(printf '\033[2m'); reset=$(printf '\033[0m')
      cyan=$(printf '\033[36m'); yellow=$(printf '\033[33m'); green=$(printf '\033[32m')

      # Info rows collected here render as a column to the right of the logo.
      info=()
      kv() { info+=("$(printf '%b%-7s%b %s' "$dim" "$1" "$reset" "$2")"); }

      info+=("$(printf '%b%btentaflake%b %bdev shell%b' "$bold" "$cyan" "$reset" "$bold" "$reset")")
      info+=("$(printf '%bisolated AI agents on NixOS · nix develop%b' "$dim" "$reset")")
      info+=("")

      # git is the contributor's own — a dev shell that pulls a second git into
      # its closure to print one line is not worth the download. No git, no rows.
      if command -v git >/dev/null 2>&1 && git rev-parse --git-dir >/dev/null 2>&1; then
        kv "branch" "$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
        subject=$(git log -1 --pretty='%h %s' 2>/dev/null || true)
        [ "''${#subject}" -gt 44 ] && subject="''${subject:0:43}…"
        kv "commit" "$subject"
        dirty=$(git status --porcelain 2>/dev/null | wc -l)
        if [ "$dirty" -eq 0 ]; then
          kv "tree" "''${green}clean''${reset}"
        else
          kv "tree" "''${yellow}$dirty uncommitted''${reset}"
        fi
      fi
      kv "nix" "$(nix --version 2>/dev/null | cut -d' ' -f3 || true)"

      # ── Render: logo left, info column right ──
      # ''${#l} counts characters, not bytes (braille is multibyte) — under a
      # non-UTF-8 locale the column just sits further right.
      mapfile -t art <<< ${lib.escapeShellArg logo}
      w=0
      for l in "''${art[@]}"; do [ "''${#l}" -gt "$w" ] && w=''${#l}; done
      pad=3 # blank rows above the info column, for rough vertical centering
      rows=''${#art[@]}
      [ $((''${#info[@]} + pad)) -gt "$rows" ] && rows=$((''${#info[@]} + pad))
      printf '\n'
      for ((i = 0; i < rows; i++)); do
        l=''${art[i]-}
        j=$((i - pad))
        if [ "$j" -ge 0 ] && [ -n "''${info[j]-}" ]; then
          printf '%b%s%b%*s   %s\n' "$cyan" "$l" "$reset" "$((w - ''${#l}))" "" "''${info[j]}"
        else
          printf '%b%s%b\n' "$cyan" "$l" "$reset"
        fi
      done

      printf '\n  %b──────────────────────────────────────────────%b\n' "$dim" "$reset"

      printf '\n  %bCOMMANDS%b\n' "$bold$cyan" "$reset"
      ${lib.concatMapStringsSep "\n" (
        c:
        "printf '    %b%-16s%b %b%s%b\\n' \"$bold\" ${lib.escapeShellArg c.cmd} \"$reset\" \"$dim\" ${lib.escapeShellArg c.what} \"$reset\""
      ) commands}

      printf '\n  %bevery commit needs a DCO sign-off:%b git commit -s\n\n' "$dim" "$reset"
    '';
  };
in
pkgs.mkShell {
  packages = with pkgs; [
    just
    nixfmt-rfc-style
    statix
    deadnix
    nil
    gotools
    golangci-lint
    shellcheck
  ];

  shellHook = ''
    ${lib.getExe banner}

    # Make it obvious which shell you are in. direnv refuses to export PS1, so
    # this only ever applies to a real `nix develop`.
    PS1='\[\033[36m\](tentaflake)\[\033[0m\] \[\033[1m\]\w\[\033[0m\] \$ '
  '';
}
