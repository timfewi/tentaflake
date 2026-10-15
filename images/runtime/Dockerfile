# The single 0.5 runtime image: Ubuntu 26.04 LTS with a pinned OpenCode,
# served over OpenCode's HTTP API. See docs/architecture.md.

# Base: the ubuntu-26.04 entry of images/catalog.json (repository@indexDigest).
FROM docker.io/library/ubuntu@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7

# Every pin of this image lives here: the apt snapshot date and the OpenCode 2
# release with the SHA-256 of its npm platform package per architecture (the
# x86-64 build is the baseline one, which runs on every VPS CPU).
ARG UBUNTU_SNAPSHOT=20261001T000000Z
ARG OPENCODE_VERSION=2.0.29
ARG OPENCODE_SHA256_AMD64=152c42ff57b15728f39e1d7b68a9c13b0635fe1c5e6dcccdc8b710955e4cef0b
ARG OPENCODE_SHA256_ARM64=0af39e75dbdfacc20f8f531ef35fb4dcc0b44c08084203518614272f232283ae
ARG TARGETARCH
ARG DEBIAN_FRONTEND=noninteractive

# apt reads a dated snapshot of the archive, so rebuilds get the same packages.
# The base image has no CA bundle yet: ca-certificates is the one package
# fetched with TLS peer checks off; apt still verifies it through the archive's
# signed Release files. The other packages are fetched with checks on.
RUN <<EOF
set -eu
rm -f /etc/apt/sources.list /etc/apt/sources.list.d/*
cat > /etc/apt/sources.list.d/snapshot.sources <<SOURCES
Types: deb
URIs: https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}
Suites: resolute resolute-updates resolute-security
Components: main universe
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
SOURCES
apt-get -o Acquire::https::Verify-Peer=false update
apt-get -o Acquire::https::Verify-Peer=false install --yes --no-install-recommends ca-certificates
apt-get update
apt-get install --yes --no-install-recommends curl git nodejs python3 ripgrep
rm -rf /var/lib/apt/lists/*
EOF

RUN <<EOF
set -eu
case "${TARGETARCH}" in
  amd64) package=cli-linux-x64-baseline; sha256=${OPENCODE_SHA256_AMD64} ;;
  arm64) package=cli-linux-arm64; sha256=${OPENCODE_SHA256_ARM64} ;;
  *) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;;
esac
curl --fail --silent --show-error --location --output /tmp/opencode.tgz \
  "https://registry.npmjs.org/@opencode/${package}/-/${package}-${OPENCODE_VERSION}.tgz"
echo "${sha256}  /tmp/opencode.tgz" | sha256sum --check --strict -
tar --extract --gzip --file /tmp/opencode.tgz --directory /usr/local/bin --strip-components=2 package/bin/opencode
rm /tmp/opencode.tgz
EOF

# The base image ships an `ubuntu` user on UID/GID 1000; the agent takes it over.
RUN <<EOF
set -eu
if getent passwd ubuntu > /dev/null; then userdel --remove ubuntu; fi
groupadd --gid 1000 agent
useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash agent
install --directory --owner agent --group agent /workspace
EOF

# OPENCODE_CONFIG is the base layer: the environment's generated configuration
# (project file, config directory or OPENCODE_CONFIG_CONTENT) overrides it.
COPY opencode.json /etc/tentaflake/opencode.json

USER agent
ENV HOME=/home/agent OPENCODE_CONFIG=/etc/tentaflake/opencode.json
WORKDIR /workspace
EXPOSE 4096

# `opencode serve` only warns without a password; the entrypoint refuses.
ENTRYPOINT ["/bin/sh", "-c", ": \"${OPENCODE_SERVER_PASSWORD:?OPENCODE_SERVER_PASSWORD is required}\"; exec opencode serve --hostname 0.0.0.0 --port 4096"]
