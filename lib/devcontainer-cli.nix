{ pkgs }:

let
  version = "0.88.0";
  upstreamSrc = pkgs.fetchFromGitHub {
    owner = "devcontainers";
    repo = "cli";
    tag = "v${version}";
    hash = "sha256-CaRZ565e+DczlEPlD0SeHg4lCMTey3anga75L79anN0=";
  };
  patchedSrc = pkgs.applyPatches {
    name = "devcontainer-cli-${version}-source";
    src = upstreamSrc;
    patches = [ ./devcontainer-cli-dependencies.patch ];
  };
in

# Keep the contributor E2E runner independently updatable while nixpkgs catches
# up. Both the release source and its complete Yarn dependency cache are pinned
# by content hash, so this remains reproducible and network-free at build time.
pkgs.devcontainer.overrideAttrs (
  finalAttrs: _previousAttrs: {
    inherit version;
    src = patchedSrc;

    yarnOfflineCache = pkgs.fetchYarnDeps {
      yarnLock = "${finalAttrs.src}/yarn.lock";
      hash = "sha256-koc0HjBGh2EdfnX3B7eU2qIkWWg2jyzbIv+baX8ebUk=";
    };

    # proxy-agent 6.5.0 still constrains proxy-from-env to the CommonJS-only 1.x
    # series. Backport the two behavior changes from proxy-from-env 2.0.0 rather
    # than forcing its ESM-only major release into a CommonJS consumer.
    postBuild = ''
      substituteInPlace node_modules/proxy-from-env/index.js \
        --replace-fail \
          "var parseUrl = require('url').parse;" \
          "function parseUrl(urlString) { try { return new URL(urlString); } catch { return null; } }" \
        --replace-fail \
          "var parsedUrl = typeof url === 'string' ? parseUrl(url) : url || {};" \
          "var parsedUrl = (typeof url === 'string' ? parseUrl(url) : url) || {};"

      # compile-prod bundles proxy-from-env, so rebuild after applying the
      # compatibility backport. The patched node_modules tree is installed too.
      yarn --offline --frozen-lockfile compile-prod
    '';
  }
)
