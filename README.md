# Tentaflake

![Tentaflake octopus and snowflake logo with agent mascots](public/tentaflake-readme.jpg)

> [!WARNING]
> **0.5 is in release preparation.** Tentaflake is pre-1.0 and was rebuilt
> from a small core. The image builds and boots, but it has not been tested on
> a real VPS provider yet (see [Verified on](#verified-on)). The previous line
> is frozen at tag `v0.4-final` and branch `legacy/0.4` and is no longer
> maintained.

Tentaflake turns a rented VPS into a private agent cloud: boot the Tentaflake
image, open the web UI, and create isolated agent environments — an Ubuntu
container in a gVisor sandbox with an agent inside, your own model API keys,
MCP servers and tools. Like an agent in the cloud, but on your own server.

You bring the VPS and the keys. Tentaflake brings the environment and the
simplicity.

## How it works

- **One host, built with NixOS.** Hardened, reproducible, updated with rollback.
- **Environments as data.** The UI creates, starts, stops and deletes
  environments; no host rebuild per environment.
- **One runtime in 0.5:** [OpenCode](https://github.com/anomalyco/opencode),
  driven over its HTTP API, with any model provider.
- **Your keys stay on the host.** Model calls go through the broker; no
  provider key enters an environment.
- **Built for teams later.** 0.5 has one admin, with a multi-user data model
  from the start.

Details and the current status: [architecture](docs/architecture.md).

## Install on a VPS

Your provider must let you upload a custom disk image. The image is a QEMU
guest with virtio disk and network (it expects its disk at `/dev/vda`), boots
with BIOS or UEFI and takes its address by DHCP; it needs a public IPv4
address on its network interface. Two vCPUs, 4 GB of memory and a 20 GB disk
are a sensible minimum; the root file system grows to the disk at the first
boot.

1. Build the image on an x86-64 Linux machine with Nix and KVM:

   ```bash
   nix build .#image-qcow2   # result/tentaflake.qcow2 (compressed)
   nix build .#image-raw     # result/tentaflake.img, for providers that want raw
   ```

   Some providers take only a compressed raw image or one at a URL; compress
   a copy with `xz -k` or `gzip -k` and upload it as the provider documents
   for custom images.
2. Create a server from the image and open the provider's console. After the
   first boot the login screen shows the address, for example
   `https://203-0-113-7.sslip.io`, the one-time setup token and a first
   password for the console login `user`. The journal
   (`journalctl -u tentaflake-setup`) shows only the address.
3. Open the address, enter the setup token, create the admin with a password
   and an authenticator app, and sign in. The certificate can take a minute.
4. Choose your own domain in the last step, or later on the Domain page:
   first point its A record to the server's IPv4 address. The UI moves to
   the new domain at once; sign in there again. Until then the name derived
   from the address is used.
5. Sign in on the console as `user` with the first password and choose your
   own. No SSH port is open; to manage the host remotely, enroll the private
   management transport with `sudo tailscale up --ssh` and use Tailscale SSH.

If the chosen domain does not work, return to the derived name on the
console with `sudo truncate -s 0 /var/lib/tentaflake/control/domain`.

## Update and roll back

The image is the flake's host `tentaflake`. Switch a running host to a newer
revision (a release tag or a commit of this repository or your fork):

```bash
sudo nixos-rebuild switch --flake github:timfewi/tentaflake/v0.5.0#tentaflake
```

Every update is a new generation. Return to the previous one with

```bash
sudo nixos-rebuild switch --rollback
```

or pick an older generation in the boot menu on the provider's console.
State lives in `/var/lib/tentaflake` and in Docker's volumes; an update keeps
it. The runtime image is rebuilt on the host only when `images/runtime`
changed; running environments keep their container until they are recreated.

Automatic updates are off. A fork can turn them on with NixOS'
`system.autoUpgrade` (`enable = true`, `flake` set to its own repository and
branch, and `allowReboot` if kernel updates should apply unattended).

## Verified on

None yet. The image is tested on real VPS providers before the `v0.5.0`
release; the providers it worked on are listed here.

## Development

```bash
nix develop
just check   # fast checks plus flake evaluation, the CI gate
```

Keep `ui/package.json` and `ui/bun.lock`'s `playwright-core` version aligned with
`playwright-driver` in the locked nixpkgs when refreshing the browser toolchain.
This development-only dependency is excluded from the packaged UI's production
dependency closure.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the recipes and conventions.

## License

MIT, see [LICENSE](LICENSE). The name and logo follow
[TRADEMARK.md](TRADEMARK.md). Report vulnerabilities as described in
[SECURITY.md](SECURITY.md).
