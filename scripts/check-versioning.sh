#!/usr/bin/env bash
# Enforce the branch versioning rule (CONTRIBUTING.md#versioned-branches).
#
#   scripts/check-versioning.sh <head-branch> <base-commit>
#
# Every change branch names the release it targets:
#   <type>/v<MAJOR>.<MINOR>.<PATCH>-<topic>   e.g. feat/v0.5.1-show-version
#   release/v<MAJOR>.<MINOR>.<PATCH>          prepares exactly that release
# The target must be the next patch, minor or major of the newest released
# CHANGELOG section on the base, and the branch must add its notes to
# CHANGELOG.md. Only release branches change the declared version, and every
# first-party version declaration must agree with Cargo.toml.
set -euo pipefail

branch=${1:?usage: check-versioning.sh <head-branch> <base-commit>}
base=${2:?usage: check-versioning.sh <head-branch> <base-commit>}

fail() {
  echo "versioning: $*" >&2
  echo "versioning: see CONTRIBUTING.md#versioned-branches" >&2
  exit 1
}

case $branch in
dependabot/* | update_flake_lock_action)
  echo "versioning: automated dependency branch '$branch' is exempt"
  exit 0
  ;;
esac

pattern='^(feat|fix|docs|chore|refactor|perf|test|ci|build|style|revert|release)/v([0-9]+)\.([0-9]+)\.([0-9]+)(-[a-z0-9][a-z0-9.-]*)?$'
[[ $branch =~ $pattern ]] ||
  fail "branch '$branch' does not match <type>/v<X.Y.Z>-<topic> or release/v<X.Y.Z>"
kind=${BASH_REMATCH[1]}
major=${BASH_REMATCH[2]} minor=${BASH_REMATCH[3]} patch=${BASH_REMATCH[4]}
topic=${BASH_REMATCH[5]}
target="$major.$minor.$patch"

if [[ $kind == release ]]; then
  [[ -z $topic ]] || fail "release branches are named release/v$target without a topic"
else
  [[ -n $topic ]] || fail "branch '$branch' needs a topic: $kind/v$target-<topic>"
fi

released_version() {
  awk 'match($0, /^## \[[0-9]+\.[0-9]+\.[0-9]+\]/) && !found { print substr($0, 5, RLENGTH - 5); found = 1 }'
}
declared_version() {
  awk '/^\[/ { section = $0 } section == "[workspace.package]" && /^version = "/ && !found { gsub(/^version = "|"$/, ""); print; found = 1 }'
}

released=$(git show "$base:CHANGELOG.md" | released_version)
[[ -n $released ]] || fail "no released version section in the base CHANGELOG.md"
IFS=. read -r rmajor rminor rpatch <<<"$released"
next_patch="$rmajor.$rminor.$((rpatch + 1))"
next_minor="$rmajor.$((rminor + 1)).0"
next_major="$((rmajor + 1)).0.0"
case $target in
"$next_patch") level="patch" ;;
"$next_minor") level="minor" ;;
"$next_major") level="major" ;;
*) fail "v$target is not the next release after v$released (v$next_patch, v$next_minor or v$next_major)" ;;
esac

git diff --name-only "$base"...HEAD -- CHANGELOG.md | grep -qx CHANGELOG.md ||
  fail "CHANGELOG.md is unchanged; add this branch's notes under [Unreleased]"

declared=$(declared_version <Cargo.toml)
base_declared=$(git show "$base:Cargo.toml" | declared_version)
if [[ $kind == release ]]; then
  grep -qE "^## \[$target\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md ||
    fail "release/v$target needs a dated '## [$target] - YYYY-MM-DD' CHANGELOG section"
  [[ $declared == "$target" ]] || fail "Cargo.toml declares $declared; release/v$target must declare $target"
else
  [[ $declared == "$base_declared" ]] ||
    fail "only release branches change the declared version ($base_declared -> $declared)"
  unreleased_entries() {
    awk '/^## / { active = ($0 == "## [Unreleased]") }
         active && /^[-*] [^[:space:]]/ { print substr($0, 3) }' | LC_ALL=C sort -u
  }
  ancestor=$(git merge-base "$base" HEAD)
  base_entries=$(git show "$ancestor:CHANGELOG.md" | unreleased_entries)
  head_entries=$(unreleased_entries <CHANGELOG.md)
  added=$(LC_ALL=C comm -13 <(printf '%s\n' "$base_entries") <(printf '%s\n' "$head_entries"))
  [[ -n $added ]] || fail "add a branch-specific entry under [Unreleased]"
fi

runner=$(sed -nE 's/^  "version": "([^"]+)",?$/\1/p' pkgs/tentaflake-pi-durable/package.json)
[[ $runner == "$declared" ]] ||
  fail "pkgs/tentaflake-pi-durable/package.json declares $runner, Cargo.toml $declared"

echo "versioning: $branch targets v$target ($level after v$released); declared version $declared"
