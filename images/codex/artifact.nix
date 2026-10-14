# Official release metadata observed 2026-10-14T02:28:23Z:
# https://api.github.com/repos/openai/codex/releases/latest (rust-v0.162.1)
# Asset 625965833, 162905879 bytes. The digest is publisher metadata;
# fetchurl verified the acquired bytes; its 54-entry full-package layout was
# inspected without execution. No signature or vendor/runtime acceptance.
{ pkgs }:
assert pkgs.lib.assertMsg (
  pkgs.stdenv.hostPlatform.system == "x86_64-linux"
) "The Codex reference artifact is pinned only for Linux x86_64.";
pkgs.fetchurl {
  name = "codex-package-0.162.1-x86_64-unknown-linux-musl.tar.gz";
  url = "https://github.com/openai/codex/releases/download/rust-v0.162.1/codex-package-x86_64-unknown-linux-musl.tar.gz";
  sha256 = "a676f5722aae0d86cbe3764332f08eab1fdd89dc0a300b3ad4ffbf48611fa3e3";
}
