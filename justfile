# tentaflake dev commands — run `just` to list them.
# Everything here mirrors CI (.github/workflows/check.yml) plus the ISO builds
# CI does NOT do, so `just ci` is a stricter local gate than the pipeline.

set shell := ["bash", "-euo", "pipefail", "-c"]

auditd := "pkgs/tentaflake-auditd"

# List recipes
default:
    @just --list

# ── The gates ────────────────────────────────────────────────

# Full local gate: everything CI runs + the two ISOs CI never builds
ci: fmt-check lint shellcheck go-lint go check iso iso-installer
    @echo "==> all green"

# nix flake check (eval + agent-host toplevel + tentaflake-auditd)
check:
    nix flake check

# Build just the agent-host system (the toplevel CI builds), no symlink
build:
    nix build .#nixosConfigurations.agent-host.config.system.build.toplevel --no-link

# ── Formatting & lint ────────────────────────────────────────

# Format the tree in place (nixfmt via `nix fmt`)
fmt:
    nix fmt

# Format check only — fails if anything is unformatted (CI mode)
fmt-check:
    nix fmt -- --ci

# Nix lint: statix (anti-patterns) + deadnix (dead bindings)
lint:
    statix check .
    deadnix --fail .

# ── Go: tentaflake-auditd ────────────────────────────────────────

# build + vet + test tentaflake-auditd (the CI Go steps)
go:
    cd {{auditd}} && go build ./... && go vet ./... && go test ./...

# golangci-lint — run before push (CLAUDE.md convention)
go-lint:
    cd {{auditd}} && golangci-lint run

# ── Shell scripts ────────────────────────────────────────────

# ShellCheck the installer + helper scripts (CI parity)
shellcheck:
    shellcheck installer/*.sh scripts/*.sh

# Preview the tentaflake-status login banner with a fake fleet (+ self-checks)
banner:
    ./scripts/banner-test.sh

# ── ISOs ─────────────────────────────────────────────────────

# Build the live-agent ISO (Hermes + Piper, boot-and-run appliance)
iso:
    ./scripts/build-iso.sh live

# Build the installer ISO (minimal, installs to disk)
iso-installer:
    ./scripts/build-iso.sh installer

# Boot the live ISO in QEMU to test runtime behavior (builds it first).
# KVM is used when /dev/kvm exists; drop nothing to test on a plain machine.
vm: iso
    qemu-system-x86_64 \
        {{ if path_exists("/dev/kvm") == "true" { "-enable-kvm" } else { "" } }} \
        -m 4096 -smp 2 \
        -cdrom result/iso/tentaflake-live.iso -boot d

# ── Release ──────────────────────────────────────────────────

# Cut a release tag. Update CHANGELOG.md FIRST, then: just tag v0.3.0
# The git tag is the source of truth for the repo version.
tag VERSION:
    @git diff --quiet || { echo "working tree dirty — commit first"; exit 1; }
    @grep -q "## \[{{ replace(VERSION, 'v', '') }}\]" CHANGELOG.md \
        || { echo "no CHANGELOG.md section for {{VERSION}} — write it first"; exit 1; }
    git tag -a {{VERSION}} -m "{{VERSION}}"
    @echo "==> tagged {{VERSION}}. Push with: git push origin {{VERSION}}"
