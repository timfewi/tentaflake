# Security Best Practices — Tentaflake
**Compiled 2026-06-22** | Repo: `tentaflake`
> Caveman-compressed reference. Every section = gap found in repo + concrete fix.

---

## 1. NixOS Security Hardening

### 1.1 Kernel Hardening

**Current state** (`modules/hardening.nix`): good baseline — kptr_restrict, dmesg_restrict, unprivileged_bpf_disabled, perf_event_paranoid, protected_{hardlinks,symlinks,fifos,regular}, userfaultfd=0, tcp_syncookies, no ICMP redirects, no source routing.

**GAPS — add these sysctls:**

```nix
boot.kernel.sysctl = {
  # --- EXISTING (good) ---
  "kernel.kptr_restrict" = 2;
  "kernel.dmesg_restrict" = 1;
  "kernel.unprivileged_bpf_disabled" = 1;
  "kernel.perf_event_paranoid" = 3;
  "vm.unprivileged_userfaultfd" = 0;
  "fs.protected_hardlinks" = 1;
  "fs.protected_symlinks" = 1;
  "fs.protected_fifos" = 2;
  "fs.protected_regular" = 2;
  "net.ipv4.tcp_syncookies" = 1;
  "net.ipv4.conf.all.accept_redirects" = 0;
  "net.ipv4.conf.default.accept_redirects" = 0;
  "net.ipv6.conf.all.accept_redirects" = 0;
  "net.ipv4.conf.all.accept_source_route" = 0;
  "net.ipv6.conf.all.accept_source_route" = 0;

  # --- ADD these ---
  "kernel.kexec_load_disabled" = 1;          # Prevent kexec
  "kernel.sysrq" = 0;                         # Disable SysRq
  "kernel.unprivileged_userns_clone" = 0;     # Restrict user namespaces (tradeoff for Docker)
  "net.ipv4.tcp_rfc1337" = 1;                 # Protect against time-wait assassination
  "net.ipv4.conf.all.rp_filter" = 1;          # Reverse path filtering
  "net.ipv4.conf.default.rp_filter" = 1;
  "net.ipv4.conf.all.arp_ignore" = 1;         # ARP filtering
  "net.ipv4.conf.all.arp_announce" = 2;
  "net.ipv4.icmp_echo_ignore_broadcasts" = 1;
  "net.ipv4.icmp_ignore_bogus_error_responses" = 1;
  "net.ipv4.conf.all.log_martians" = 1;       # Log spoofed/deformed packets
  "net.ipv6.conf.all.disable_ipv6" = 0;       # Keep ipv6 if needed, but:
  "net.ipv6.conf.all.autoconf" = 0;           # Disable SLAAC if static
  "net.ipv6.conf.all.accept_ra" = 0;          # Disable router advertisements
  "net.core.bpf_jit_harden" = 2;              # JIT hardening
};
```

**⚠️ NOTE:** `user.max_user_namespaces = 1000` is already set in hardening.nix. This is GOOD — Docker requires unprivileged user namespaces. Value of 1000 is reasonable for multi-agent.

**Add kernel hardening params to cmdline:**

```nix
boot.kernelParams = [
  "slab_nomerge"           # Prevent heap overflow merging
  "init_on_alloc=1"        # Zero memory on alloc
  "init_on_free=1"         # Zero memory on free
  "page_alloc.shuffle=1"   # Shuffle page alloc freelist
  "pti=on"                 # Page Table Isolation (KPTI)
  "randomize_kstack_offset=on"
  "vsyscall=none"          # Disable legacy vsyscall
  "debugfs=off"            # Disable debugfs
];
```

**Check LSM order:**
```nix
boot.kernelParams = [ "lsm=landlock,lockdown,yama,integrity,apparmor,bpf" ];
```
Current repo enables AppArmor but does NOT set LSM order. Without explicit order, apparmor might not be first in enforcement chain.

### 1.2 Nix Daemon Security

**Current state** (`modules/nix-settings.nix`):
- `trusted-users = ["root", adminUser]` — GOOD
- No `allowed-users` set — **GAP**: defaults to `*` (everyone can connect to daemon)
- No `restrict-eval` — **GAP**
- No `sandbox` settings — **GAP**

**Harden:**
```nix
nix.settings = {
  # --- EXISTING ---
  experimental-features = [ "nix-command" "flakes" ];
  trusted-users = [ "root" (params.adminUser or "admin") ];

  # --- ADD ---
  allowed-users = [ "@wheel" ];                    # Only wheel group can evaluate
  sandbox = true;                                   # Always sandbox builds
  sandbox-fallback = false;                         # Fail if sandbox can't start
  restrict-eval = true;                             # Path/eval restrictions
  allowed-uris = [ "https://github.com" "https://cache.nixos.org" ];
  require-sigs = true;                             # Require signatures on substituter paths
  tarball-ttl = 3600;                               # Cache tarballs 1hr
  max-build-jobs = "auto";
  builders-use-substitutes = true;
  min-free = "2G";                                  # GC when free < 2G
  max-free = "8G";                                  # GC until free > 8G
};
```

**IFD (Import From Derivation) risk:**
- Repo has NO IFD currently. Good.
- IFD blocked by default in flakes since Nix 2.16+.
- If IFD needed: `nix.settings.allow-import-from-derivation = true;` — but DON'T enable unless required.
- IFD risk: eval pauses, arbitrary builds during eval, trusted user can bypass.

### 1.3 Secure Boot & Measured Boot

**Current state** (`modules/boot.nix`): `systemd-boot.enable = true;` — basic, no secure boot, no TPM.

**Production hardening — add Lanzaboote + TPM2:**
```nix
# flake.nix inputs:
lanzaboote = {
  url = "github:nix-community/lanzaboote";
  inputs.nixpkgs.follows = "nixpkgs";
};

# In NixOS module:
boot.loader = {
  systemd-boot.enable = lib.mkForce false;   # Disable sd-boot
  efi.canTouchEfiVariables = true;
  lanzaboote = {
    enable = true;
    pkiBundle = "/etc/secureboot";            # Generated via lzbt CLI
  };
};

# TPM2 LUKS unlock (if FDE):
boot.initrd.luks.devices."luks-root" = {
  device = "/dev/disk/by-partlabel/root";
  allowDiscards = true;
  crypttabExtraOpts = [
    "tpm2-device=auto"
    "tpm2-measure-pcr=yes"
    "tpm2-pcrs=0+2+7+12"
  ];
};
```
**⚠️ WARNING:** `tpm2-pcrs=0+2+7` only protects firmware + secure boot policy + shim. Add PCR 12 to also measure kernel command line. See [oddlama's blog](https://oddlama.org/blog/bypassing-disk-encryption-with-tpm2-unlock/) — without PCR 12 + LUKS identity verification, TPM unlock can be bypassed by physical attacker (Jan 2025).

**LUKS key management:**
- Keep ONE password-based slot as backup (slot 0).
- Never delete all password slots.
- BIOS password = required for secure boot to be meaningful (prevents USB boot override).

### 1.4 Boot Loader Protection

- GRUB not used (systemd-boot). No GRUB password to set.
- systemd-boot doesn't support password protection natively.
- Secure Boot (via Lanzaboote) + BIOS password = the way to protect boot chain on NixOS.
- Add `boot.loader.systemd-boot.editor = false;` to prevent kernel param editing at boot menu.

### 1.5 PAM & Login Security

**Add to repo — not present currently:**
```nix
security = {
  # Lock root account (use sudo instead)
  denyRootLogin = true;

  # PAM lockout after 3 failed attempts
  pam = {
    enableAppArmor = true;    # Not the same as apparmor.enable
    services.sudo.otpw = {};  # or use:
    # Enforce password quality
    services.login.makeHomeDir = false;  # Prevent auto-home creation
  };

  # SSH hardening
  services.openssh = {
    enable = true;
    settings = {
      PasswordAuthentication = false;    # Key-only
      KbdInteractiveAuthentication = false;
      PermitRootLogin = "prohibit-password";
      MaxAuthTries = 3;
      MaxSessions = 10;
      ClientAliveInterval = 300;
      ClientAliveCountMax = 2;
      AllowUsers = [ (params.adminUser or "admin") ];
    };
  };

  # Fail2ban
  services.fail2ban.enable = true;
};
```

### 1.6 AppArmor on NixOS — Current Reality (2026)

**Current state:** `security.apparmor.enable = true;` in hardening.nix. BUT:

**REALITY CHECK (April 2026):**
- AppArmor **NOT properly integrated** into NixOS yet. See [NixOS Discourse](https://discourse.nixos.org/t/apparmor-on-nixos-roadmap/57217/26).
- GSoC 2026 project proposed to fix this. As of June 2026, may still be WIP.
- Enabling AppArmor sets kernel param, loads `apparmor` kernel module, but profile management is manual.
- Pre-built AppArmor profiles from other distros may not work with NixOS paths.
- **Alternative:** Use systemd service hardening (`serviceConfig`) instead — this IS production-ready on NixOS.

**Do this instead of AppArmor for agent services:**
```nix
systemd.services."docker-hermes-<name>" = {
  serviceConfig = {
    ProtectSystem = "strict";           # Read-only /usr /etc
    ProtectHome = true;                  # No /root /home
    PrivateTmp = true;
    PrivateDevices = true;
    ProtectKernelTunables = true;
    ProtectKernelModules = true;
    ProtectControlGroups = true;
    MemoryMax = "2G";                    # Per-service memory limit
    TasksMax = 100;                      # Max processes
    RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
    RestrictNamespaces = true;
    CapabilityBoundingSet = "";          # No capabilities
    SystemCallFilter = [ "@system-service" "~@privileged" ];
    NoNewPrivileges = true;
    LockPersonality = true;
    RestrictRealtime = true;
    RemoveIPC = true;
  };
};
```
Huge impact. These are enforced by systemd, not dependent on AppArmor upstream.

### 1.7 Nix Store Protection

**Current:** No special store protection.

**Add:**
```nix
nix.settings = {
  # Prevent accidental deletion of live store paths
  keep-derivations = true;
  keep-outputs = true;                     # Keep used derivations

  # Auto-optimize (already in nix-settings.nix — good)
  auto-optimise-store = true;

  # Restrict store directory permissions
};
```
**Store leakage via world-readable:**
- Nix store is world-readable by default.
- Hidden in `build-sandbox-paths` — no granular ACLs.
- For secrets: NEVER put unencrypted secrets in Nix store. Use agenix/sops-nix.

---

## 2. Container/Docker Security for AI Agent Workloads

### 2.1 Current State in Repo — RISK ASSESSMENT

**configuration.nix:**
```nix
virtualisation.docker.enable = true;
users.users.${params.adminUser}.extraGroups = [ "docker" ];  # ⚠️ HIGH RISK
```

**RISK:** Docker group = root-equivalent on host. [Well-documented escalation path](https://knowledge-base.secureflag.com/vulnerabilities/broken_authorization/privilege_escalation_docker.html):
```
docker run -v /:/host -it ubuntu chroot /host
```
Any process running as admin user can trivially get host root.

**Mitigations (choose one):**

**Option A — Rootless Docker (recommended for multi-agent):**
```nix
virtualisation.docker = {
  enable = true;
  rootless = {
    enable = true;
    setSocketVariable = true;   # Sets DOCKER_HOST for user
  };
  # Remove admin from docker group — rootless doesn't need it
  daemon.settings = {
    # Rootless-specific config
    "user" = (params.adminUser or "admin");
  };
};
```
**⚠️ Rootless tradeoffs:**
- No `--network=host` on most kernels (uses slirp4netns, ~5-10% perf hit)
- No `--privileged` containers
- No overlayfs (uses fuse-overlayfs)
- BuildKit needs `seccomp=unconfined` inside containers
- AppArmor on Ubuntu 24.04+ blocks unprivileged userns by default — need `kernel.apparmor_restrict_unprivileged_userns=0`

**Option B — userns-remap (keep rootful daemon, still isolate):**
```nix
virtualisation.docker = {
  enable = true;
  daemon.settings = {
    "userns-remap" = "default";     # Remap UIDs to subuid range
  };
};
```
⚠️ userns-remap breaks volume mounts (file ownership mismatch). Don't use if host volume mounts are critical.

**Option C — Keep rootful but audit docker group:**
```nix
# Accept risk, but document and watch
users.users.${params.adminUser}.extraGroups = [ "docker" ];
```
Accept if single-user machine, no untrusted access. But MANAGE RISK.

### 2.2 Seccomp Profiles

**Current:** Docker default seccomp profile — reasonable for most workloads.

**Add custom seccomp for agent containers:**
```nix
virtualisation.docker.daemon.settings = {
  "seccomp-profile" = "/etc/docker/seccomp-agent.json";
};
```
Create profile dropping syscalls agents don't need:
- `mount`, `umount2` — prevent filesystem mounting
- `ptrace` — prevent process tracing
- `perf_event_open` — prevent performance monitoring
- `add_key`, `request_key` — prevent kernel keyring abuse

### 2.3 Per-Container Resource Limits

**Current:** Only memory limit set in `live-agents.nix` (`--memory=2g` / `--memory=4g`). 

**Add these to mkHermesAgent's extraContainerConfig:**
```nix
extraContainerConfig = {
  extraOptions = [
    "--network=host"          # Required for Hermes gateways
    "--memory=2g"             # Already set — good
    "--memory-swap=2g"        # No swap for container
    "--cpus=2"                # CPU limit
    "--pids-limit=200"        # Prevent fork bomb (Docker default = 0 = UNLIMITED!)
    "--read-only"             # Read-only root filesystem
    "--security-opt=no-new-privileges:true"
    "--cap-drop=ALL"          # Drop all capabilities
    "--cap-add=NET_BIND_SERVICE"  # Only what's needed
    "--security-opt=seccomp=/etc/docker/seccomp-agent.json"
  ];
};
```

**CRITICAL:** `--pids-limit` — default Docker allows unlimited fork. AI agents run LLM subprocesses that could forkbomb.

### 2.4 Volume Mount Risks

**Current mkHermesAgent mounts:**
```nix
volumes = [
  "${stateDir}:${stateDir}:rw"
  # config.yaml mounted ro
  "${configYaml}:${stateDir}/config.yaml:ro"
];
```

**ADD** `:rslave` propagation or `:rprivate` to prevent mount propagation attacks:
```nix
volumes = [
  "${stateDir}:${stateDir}:rw,rprivate"
];
```

**Nix store access from containers:**
- Docker containers see host /nix/store by default (bind-mounted from overlay).
- Agent can read all Nix store paths.
- **Mitigation:** Set `"no-new-privileges": true` AND drop `CAP_SYS_ADMIN`.
- If rootless Docker: no access to host /nix/store.

### 2.5 Network Isolation for Containers

**Current:** `--network=host` — REQUIRED for Hermes gateways (Telegram, Discord webhooks need localhost access).

**RISK:** Host networking = full host network namespace. No network isolation.

**Mitigating with iptables/nftables:**
```nix
networking.nftables.enable = true;
networking.firewall = {
  enable = true;
  allowedTCPPorts = [ 80 443 ];   # Only needed ports
  # Block container→host traffic except on needed ports
  extraCommands = ''
    iptables -I FORWARD -i docker0 -o docker0 -j DROP
    iptables -I FORWARD -o docker0 -j DROP
    iptables -I FORWARD -i docker0 -j DROP
  '';
};
```
But with `--network=host`, container iptables rules ARE host rules. ⚠️

**Better approach:** If agents only need outbound + gateway ports, use `--network=bridge` with published ports:
```nix
extraContainerConfig = {
  extraOptions = [
    "--network=bridge"
    "-p=127.0.0.1:9090:9090"   # auditd port only
    "-p=127.0.0.1:5001:5001"   # TTS port only
  ];
};
```

---

## 3. AI Agent Security (Hermes-Specific)

### 3.1 Agent Sandboxing

**Hermes has 7-layer defense-in-depth:**
1. User allowlists
2. Dangerous command approval
3. Docker/sandbox isolation
4. MCP credential filtering
5. Prompt injection scanning
6. Cross-session isolation
7. Input sanitization

**Current repo config per agent (`live-agents.nix`):**
```nix
approvals.mode = "smart";                  # Auto-approve safe, flag dangerous
terminal.backend = "none";                 # Disable terminal — good for security
web.backend = "firecrawl";                 # Web via Firecrawl API only
toolsets = [ "terminal" "web" "memory" "file" "skills" ];
```
**Terminal disabled** = significantly reduced attack surface. Agent can't run arbitrary shell commands.

**For production agents needing terminal:**
```nix
terminal.backend = "docker";   # NOT "local" — container isolation
# NOT "none" if you need commands
```

**Add tool output limits (good defaults exist in config):**
```nix
tool_output = {
  max_bytes = 150000;          # Already in config
  max_lines = 5000;            # Already in config
};
file_read_max_chars = 100000;  # Already in config
agent.max_turns = 50;          # Already set — prevents runaway loops
```

**IMPORTANT:** `max_turns = 50` is REASONABLE but deprecating. See Hermes docs for per-loop token budgets.

### 3.2 API Key Management for LLM Providers

**Current:** Env files at `/run/hermes/<name>.env` (tmpfs) — created by firstboot wizard.

**RISK:** Plaintext API keys in environment variables. Accessible via `docker inspect` for any user in docker group.

**Mitigation — use agenix:**
```nix
# In agents config:
(mkHermesAgent {
  name = "default";
  agenixFile = "/run/agenix/hermes-default-env";
  # NOT envFile = "/run/hermes/default.env";
})
```

**Agenix integration:**
```nix
# flake.nix — uncomment:
agenix = {
  url = "github:ryantm/agenix";
  inputs.nixpkgs.follows = "nixpkgs";
};

# secrets.nix:
let
  systemKey = "ssh-ed25519 AAAAC3...";
in {
  "hermes-default-env.age".publicKeys = [ systemKey ];
}
```

**API key rotation schedule:**
- Set calendar reminder every 90 days.
- Use multiple provider API keys for fallback (already configured in agent settings).
- Never commit `.env` files — `.gitignore` must include `*.env` (check repo).

### 3.3 MCP Server Security

**Hermes MCP security features:**
- Filtered environment variables for MCP subprocesses (no credential leakage).
- Sanitized error messages before returning to LLM.
- Tool filtering possible via configuration.

**Current repo:** No MCP servers configured. When adding:

```yaml
# In agent settings:
mcp_servers:
  # Limit env vars passed to MCP:
  filesystem:
    env_filter:
      - "AWS_*"       # Block AWS creds
      - "OPENAI_*"    # Block LLM creds  
      - "HERMES_*"    # Block agent config
    allowed_tools:
      - "read_file"   # Explicit allowlist
      - "write_file"
```

**MCP threat:**
- Every MCP server expands trust boundary.
- Compromised MCP server = attacker controls agent instructions.
- Validate MCP server provenance before connecting.

### 3.4 Prompt Injection Mitigation at Infrastructure Level

**Hermes built-in:**
- Context-file scanning for injection patterns
- Input sanitization
- Tirith security scanning (optional)

**Add config:**
```yaml
security:
  prompt_injection:
    enabled: true
    scan_context_files: true
    scan_external_content: true
    max_context_size: 100000    # Truncate large context
    block_known_patterns: true
```

**Infrastructure-level:**
- Rate-limit inbound messages to agent gateways.
- Max message length limits.
- Audit ALL agent responses for potential injection.

### 3.5 Audit Logging for Agent Actions

**Current repo:** `hermes-auditd` Go daemon — watches `/var/lib/hermes-*/` for filesystem events.

**Good foundation but missing:**
- HTTP API (not yet implemented — `notifyCh` discarded in main.go)
- Agent action audit (prompts, tool calls, responses)
- Structured log aggregation

**Add:**
```nix
# systemd-journald forwarding to central syslog
services.journald.extraConfig = ''
  ForwardToSyslog=yes
  MaxLevelStore=warning
'';

# Or use auditd integration
security.auditd.enable = true;
```

---

## 4. CI/CD Security for Nix Projects

### 4.1 Current CI Pipeline

**Current** (`.github/workflows/check.yml`):
- `nix flake check --no-build` — basic flake validation
- `go test ./...` — unit tests

**Weaknesses:**
- No dependency vulnerability scanning
- No SAST
- No SBOM generation
- No container image scanning

### 4.2 Dependency Scanning for Nix

**Add to CI:**
```yaml
- name: Audit flake.lock
  run: |
    nix run github:determinatesystems/nix-flake-audit
    # Scans for known-vulnerable inputs

- name: Trivy scan (filesystem)
  uses: aquasecurity/trivy-action@master
  with:
    scan-type: fs
    scan-target: .
    format: sarif
    output: trivy-results.sarif
    severity: HIGH,CRITICAL

- name: Upload Trivy results
  uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: trivy-results.sarif
```

**⚠️ NOTE:** As of March 2026, Trivy's release infra was supply-chain compromised. Database updates suspended. **Alternative: Grype** (Anchore) — comparable, not compromised.

```yaml
- name: Grype scan
  uses: anchore/scan-action@v3
  with:
    path: "."
    fail-build: true
    severity-cutoff: critical
```

### 4.3 flake.lock Audit Automation

**Add:** `nix flake check --all` already runs. But separately:
```yaml
- name: Check for outdated flake inputs
  run: |
    nix flake update --dry-run 2>&1 | head -50
    # If you want strict: nix flake check --override-input ... enforcement

- name: Flake audit
  run: |
    nix run github:determinatesystems/flake-audit || true
    # Reports CVEs in nixpkgs inputs
```

### 4.4 SAST for Nix Expressions

**Limited tooling exists:**
- `nix flake check` — basic syntax, no security analysis.
- `statix` — linter for Nix expressions (`nix run github:nerdypepper/statix`).
- `nixfmt` — formatter (already in flake.nix as formatter).

**Semgrep for Nix:** Limited. Write custom rules for:
- Hardcoded secrets in Nix files
- allowUnfree = true (already set in repo)
- insecure package usage

### 4.5 Container Scanning

**Add to CI after ISO builds:**
```yaml
- name: Build container
  run: docker build -t hermes-agent:test .

- name: Scan container
  uses: aquasecurity/trivy-action@master
  with:
    scan-type: image
    scan-target: hermes-agent:test
```

**For the hermes-agent container itself:**
- Official `nousresearch/hermes-agent:latest` is pulled at runtime.
- Pin to a specific digest, not `:latest`.
```nix
image = "nousresearch/hermes-agent@sha256:abc123...";
```

---

## 5. Secret Management Patterns

### 5.1 Current State

**Repo:** agenix input commented out in flake.nix. Secrets handled via:
- `/run/hermes/<name>.env` (tmpfs) — for ISOs
- `envFile = "/run/secrets/hermes-<name>.env";` — for production

**No agenix, no sops-nix, no git-crypt — GAP for production deployments.**

### 5.2 Agenix vs sops-nix vs git-crypt

| Feature | Agenix | sops-nix | git-crypt |
|---------|--------|----------|-----------|
| Encrypts files individually | ✅ Per-file | 🔶 Multi-doc per file | ❌ Whole repo |
| Key type | SSH (age) | PGP / age | PGP |
| NixOS module | ✅ | ✅ | ❌ |
| Activation time | System activation | System activation | Git only |
| Supports binary secrets | ❌ | ✅ | ✅ |
| Error-prone | Low | Medium (sops.yaml sync) | High (merge conflicts) |
| Pre-boot (initrd) | ❌ | ❌ | ❌ |

**Recommendation: Agenix** for this repo — simpler, more Nix-native, SSH key based.

### 5.3 Agenix Best Practices

**Setup:**
```nix
# flake.nix
agenix = {
  url = "github:ryantm/agenix";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

**Key management:**
```nix
# secrets.nix
let
  adminKeys = [
    "ssh-ed25519 AAAAC3...admin"     # Host key
    "ssh-ed25519 AAAAC3...backup"    # Backup/admin access
  ];
in {
  "hermes-default-env.age".publicKeys = adminKeys;
  "hermes-research-env.age".publicKeys = adminKeys;
}
```

**Key rotation:**
- Add new key FIRST, re-encrypt, verify, remove old key.
- Keep at least 2 keys (primary + recovery).
- Store recovery key offline.

**Never:**
- Put `age` private keys in Nix store.
- Use agenix for initrd secrets (use LUKS + TPM instead).
- Commit unencrypted `.age` files (`.gitignore` should have `!*.age` — store encrypted files).

### 5.4 .env File Alternatives

**For ISOs** (current approach — acceptable because tmpfs):
- `/run/hermes/` is tmpfs, data in RAM only.
- Firstboot wizard prompts for keys.
- USB fallback: `HERMES_ENV`-labeled USB with `.env` files.
- **RISK:** USB can be stolen. Consider encrypting USB.

**For production:**
- Use agenix → `/run/agenix/<name>-env` (mounted only at activation).
- Never fall back to plain `.env` files on disk.

### 5.5 Nix Store Secret Leakage Prevention

**Golden rule:** If a string goes through `${...}` in a Nix expression, it's in the Nix store. World-readable.

**Checklist:**
- ✅ Repo uses `envFile`/`agenixFile` paths — secrets never hit Nix store.
- ✅ Settings in Nix config (model names, etc.) are not secrets.
- ❗ BUT: `mkHermesAgent` generates `${configYaml}` — this is built at eval time. If settings contain secrets, they're in /nix/store.
- **Fix:** Keep only non-secret settings in Nix config. Use env vars for API keys.

### 5.6 Service-Specific Secrets

**For `hermes-auditd`:**
```nix
# Don't hardcode DB path in config.go default
# The default "/var/lib/hermes-audit/events.db" is fine — not secret.
# But consider: environment variable injection for DB creds.
```
**auditd has NO auth/API keys in code** — good. But when HTTP API is added:
- Require API key for read endpoints.
- Mount API key via agenix.
- Never store in default config.

---

## 6. Go Daemon Security (hermes-auditd)

### 6.1 Current Code Audit

**`cmd/hermes-auditd/main.go`:**
- JSON structured logging — ✅
- Signal handling (SIGINT, SIGTERM) — ✅
- Context-based cancellation — ✅
- 200ms sleep after cancel — ⚠️ insufficient for flush

**`internal/store/store.go`:**
- Parameterized queries — ✅ (all `?` placeholders, no string concat for values)
- WAL mode — ✅
- MaxOpenConns(1) — ✅
- No raw SQL from user input — ✅ (agent/file/op are programmer-supplied)
- **QUERY function:** `buildQuery` uses string building for column names (`agent`, `timestamp`, etc.) — ⚠️ column names are hardcoded, not user-supplied, so OK.

**`internal/watcher/watcher.go`:**
- fsnotify file watching — ✅
- Debouncing — ✅
- Path traversal? `agentNameFromPath` strips prefix, splits on `/` — ✅ safe
- `isIgnored` checks `.git`, `node_modules`, SQLite aux files — ✅

### 6.2 SQL Injection Prevention

**Current state:** Parameterized queries everywhere. `buildQuery` constructs SQL with hardcoded column names. **No SQL injection vector.**

**But when more query features added:**
```go
// NEVER do this:
query := fmt.Sprintf("SELECT * FROM events WHERE %s = '%s'", userColumn, userValue) // ❌

// ALWAYS use parameterized:
query := `SELECT * FROM events WHERE ` + sanitizedColumn + ` = ?`  // ✅ if column is from known-set
```

**Add column allowlist when dynamic column name is needed:**
```go
var allowedColumns = map[string]bool{
  "agent": true, "file": true, "op": true, "timestamp": true, "size": true,
}

func sanitizeColumn(col string) string {
  if allowedColumns[col] {
    return col
  }
  return "id"  // safe default
}
```

### 6.3 SQLite-Specific Hardening

**Current PRAGMAs:**
```sql
PRAGMA journal_mode = WAL;      -- ✅ Good for concurrency
PRAGMA synchronous = NORMAL;    -- ✅ Good for WAL
PRAGMA busy_timeout = 5000;     -- ✅ Good
```

**Add these PRAGMAs (in schema.go):**
```sql
PRAGMA secure_delete = ON;      -- Overwrite deleted data
PRAGMA temp_store = MEMORY;     -- Don't leave temp files
-- DON'T add these for WAL mode:
-- PRAGMA foreign_keys = ON;    -- events table has no FK references
```

**Add defensive config (in store.go New):**
```go
// After opening DB, set SQLite limits
db.Exec("PRAGMA max_page_count = 10000")     // ~80MB max DB size
db.Exec("PRAGMA mmap_size = 268435456")       // 256MB mmap limit
```

### 6.4 Resource Limits / DoS Protection

**Current:**
- `events` table has no size constraint.
- No max query result limit enforcement beyond code default of 100.
- PruneLoop runs every 10min, prunes by retention time (configurable).

**GAPS:**
- No max DB size guard.
- No query timeout.
- No concurrent request limiting (HTTP server not yet implemented).

**Add:**
```go
// In Query function — already limits to 100, but HARD cap:
const maxLimit = 1000
if limit > maxLimit {
  limit = maxLimit
}

// In main.go — add HTTP server with:
srv := &http.Server{
  Addr:         fmt.Sprintf(":%d", cfg.Port),
  ReadTimeout:  5 * time.Second,
  WriteTimeout: 10 * time.Second,
  IdleTimeout:  60 * time.Second,
}
```

### 6.5 File Descriptor Safety

**Current:**
- SQLite: `SetMaxOpenConns(1)`, `SetMaxIdleConns(1)` — ✅ fine.
- fsnotify: single watcher, no limit on watched files.
- **RISK:** `addRecursive` watches every directory under each watch dir. If agent creates deep directory tree, FD usage grows unbounded.

**Mitigation:**
```go
// In NewWatcher — set max watched dirs:
const maxWatchedDirs = 10000
if dirCount > maxWatchedDirs {
  return nil, fmt.Errorf("too many watched dirs: %d > %d", dirCount, maxWatchedDirs)
}
```
**Linux default:** `fs.inotify.max_user_watches = 8192` (sysctl). Repo's `/var/lib/hermes-*/workspace/` could hit this if agents create many files.

**Sysctl:**
```nix
boot.kernel.sysctl = {
  "fs.inotify.max_user_watches" = 65536;   # Increase for hermes-auditd
  "fs.inotify.max_user_instances" = 512;
};
```

### 6.6 Proper Signal Handling

**Current:** Good pattern — `signal.Notify(sigCh, SIGINT, SIGTERM)`. Cancel context → wait 200ms → exit.

**Improvements:**
```go
// 1. Add SIGHUP for config reload (future):
signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM, syscall.SIGHUP)

// 2. Add proper flush on shutdown:
sig := <-sigCh
slog.Info("shutting down", "signal", sig)
cancel()

// Wait for flush with timeout
flushDone := make(chan struct{})
go func() {
  // Wait for pending inserts to complete
  st.Flush()  // Future method — flush pending writes
  close(flushDone)
}()

select {
case <-flushDone:
  slog.Info("flush completed")
case <-time.After(5 * time.Second):
  slog.Warn("flush timeout, forcing exit")
}
```

**Current 200ms sleep:** Too short. SQLite WAL flush might not complete. Make configurable:
```go
const shutdownGracePeriod = 5 * time.Second
time.Sleep(shutdownGracePeriod)
```

### 6.7 Missing Error Handling

- `store.Insert`: logs error, continues — ✅ good pattern.
- `store.Prune`: logs error but doesn't return — minor.
- **watcher toEvent:** `os.Stat` error silently returns size=0 — acceptable (file may be gone).
- **isIgnored:** no path traversal check — ✅ uses `filepath.Separator` splitting, safe.
- **main.go:** no `recover()` — panics will crash daemon. Consider:
```go
defer func() {
  if r := recover(); r != nil {
    slog.Error("panic recovered", "recover", r)
    os.Exit(2)
  }
}()
```

---

## Priority Action Items

### 🔴 CRITICAL (fix immediately)
1. `users.${adminUser}.extraGroups = [ "docker" ]` — mitigations: rootless Docker OR justify + document risk
2. Add `--pids-limit` + `--cap-drop=ALL` + `--read-only` to container configs
3. Add `allowed-users = [ "@wheel" ]` and `sandbox = true` to nix-settings.nix
4. Add `boot.kernelParams` for slab_nomerge, init_on_alloc/free, pti=on
5. Add `kernel.kexec_load_disabled` + `kernel.sysrq=0` to sysctls

### 🟡 HIGH (next sprint)
6. Enable Lanzaboote + TPM2 LUKS unlock (for physical deployments)
7. Add Grype/Trivy scanning to CI pipeline
8. Uncomment agenix integration — move env vars to age-encrypted files
9. Add SQLite secure_delete + max_page_count PRAGMAs
10. Add `systemd.services.docker-hermes-*.serviceConfig` hardening

### 🟢 MEDIUM (tech debt)
11. Add `fs.inotify.max_user_watches` sysctl for hermes-auditd
12. Increase shutdown grace period from 200ms to 5s
13. Add query timeout + result cap enforcement in store.go
14. Add column allowlist for future dynamic query building
15. Pin container image digest instead of `:latest`
16. Add Semgrep/statix linting to CI

---

## References

- [NixOS Security Wiki](https://wiki.nixos.org/wiki/Security)
- [Nix Reference Manual — nix.conf](https://nix.dev/manual/nix/2.19/command-ref/conf-file)
- [CIS Benchmarks](https://www.cisecurity.org/cis-benchmarks) (use Level 1 for server baseline)
- [Lanzaboote — Secure Boot for NixOS](https://github.com/nix-community/lanzaboote)
- [Bypassing TPM2 LUKS Unlock](https://oddlama.org/blog/bypassing-disk-encryption-with-tpm2-unlock/)
- [Docker Rootless Mode Docs](https://docs.docker.com/engine/security/rootless/)
- [Hermes Agent — Security Docs](https://hermes-agent.nousresearch.com/docs/user-guide/security)
- [Hermes Agent — 7-Layer Defense](https://www.hostinger.com/tutorials/hermes-agent-security)
- [Agenix](https://github.com/ryantm/agenix) — recommended secrets management
- [Trivy Supply Chain Incident (March 2026)](https://codenote.net/en/posts/trivy-tfsec-alternatives-security-scanning-tools-comparison/)
- [SQLite Security](https://www.sqlite.org/security.html)
- [OWASP Docker Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Docker_Security_Cheat_Sheet.html)
- [NixOS AppArmor Roadmap (Discourse)](https://discourse.nixos.org/t/apparmor-on-nixos-roadmap/57217/26)
