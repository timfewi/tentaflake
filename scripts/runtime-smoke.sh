#!/usr/bin/env bash
# Smoke-test the runtime image: build it, run it hardened (under gVisor when
# Docker has the runsc runtime), and drive OpenCode's HTTP API. No model call,
# no API key. Needs Docker; see `just runtime-smoke`.
set -euo pipefail

cd "$(dirname "$0")/.."

image=${RUNTIME_IMAGE:-tentaflake-runtime:smoke}
name=tentaflake-runtime-smoke-$$
volume=$name
version=$(sed -n 's/^ARG OPENCODE_VERSION=//p' images/runtime/Dockerfile)
OPENCODE_SERVER_PASSWORD=$(openssl rand -hex 16)
export OPENCODE_SERVER_PASSWORD

runtime=()
if docker info --format '{{json .Runtimes}}' | grep -q '"runsc"'; then
  runtime=(--runtime=runsc)
else
  echo "WARNING: Docker has no runsc runtime; using the default runtime." >&2
  echo "WARNING: this does not show that the image runs under gVisor." >&2
fi

# The restrictions the control service applies to an environment.
hardening=(
  "${runtime[@]}"
  --user 1000:1000 --cap-drop=ALL --security-opt=no-new-privileges
  --read-only --tmpfs "/tmp:exec,mode=1777" --tmpfs "/home/agent:exec,uid=1000,gid=1000,mode=0700"
  --memory 2g --pids-limit 512
)

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

cleanup() {
  local status=$?
  if ((status != 0)); then docker logs --tail 50 "$name" >&2 || true; fi
  docker rm --force --volumes "$name" >/dev/null 2>&1 || true
  docker volume rm --force "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# Authenticated request; the password stays out of the process list.
api() {
  curl --fail --silent --show-error --max-time 10 \
    --config <(printf 'user = "opencode:%s"\n' "$OPENCODE_SERVER_PASSWORD") "$@"
}

# HTTP status of an unauthenticated request.
status() { curl --silent --output /dev/null --write-out '%{http_code}' --max-time 10 "$@" || true; }

docker build --tag "$image" images/runtime

# Without a password the container must refuse to start (a hang would be 124).
rc=0
refusal=$(timeout 60 docker run --rm "${hardening[@]}" "$image" 2>&1) || rc=$?
if ((rc == 0 || rc == 124)) || ! grep -q OPENCODE_SERVER_PASSWORD <<<"$refusal"; then
  fail "container did not refuse to start without a password (exit $rc)"
fi
echo "ok: refuses to start without OPENCODE_SERVER_PASSWORD"

docker volume create "$volume" >/dev/null
docker run --detach --name "$name" "${hardening[@]}" \
  --volume "$volume":/workspace --publish 127.0.0.1::4096 \
  --env OPENCODE_SERVER_PASSWORD "$image" >/dev/null
base=http://$(docker port "$name" 4096/tcp | sed -n '1p')

for _ in $(seq 60); do
  api "$base/global/health" >/dev/null 2>&1 && break
  [[ $(docker inspect --format '{{.State.Running}}' "$name") == true ]] || fail "container exited"
  sleep 1
done
health=$(api "$base/global/health")
grep -q "\"version\":\"$version\"" <<<"$health" || fail "unexpected health response: $health"
echo "ok: server ready, OpenCode $version (${runtime[0]:-default runtime})"

[[ $(status "$base/global/health") == 401 ]] || fail "unauthenticated health request was not refused"
[[ $(status --request POST --header 'content-type: application/json' --data '{}' "$base/session") == 401 ]] ||
  fail "unauthenticated session request was not refused"
echo "ok: requests without the password are refused"

api --request POST --header 'content-type: application/json' --data '{}' "$base/session" |
  python3 -I -c 'import json, sys; assert json.load(sys.stdin)["id"].startswith("ses_")'
echo "ok: authenticated request created a session"

api "$base/config" | python3 -I -c '
import json, sys
config = json.load(sys.stdin)
assert config["share"] == "disabled" and config["autoupdate"] is False, config
assert config["permission"]["bash"] == "ask" and config["permission"]["webfetch"] == "ask", config'
echo "ok: effective configuration has sharing and auto-update off"

docker exec "$name" touch /workspace/.smoke
echo "ok: workspace volume is writable by the agent user"
