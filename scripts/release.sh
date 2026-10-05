#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# The one place a cox release version is decided (bump.yml runs it).
#
#   scripts/release.sh [patch|minor|major] [--dry-run|--local]
#
#   (none)     start the Bump workflow (gh workflow run bump.yml -f level=...)
#   --dry-run  print the version that would be released; change nothing
#   --local    make the version commit and stop (what bump.yml runs)
#
# The version in [workspace.package] is the version to release; it is raised
# only when that version is already tagged. The commit carries Cargo.toml,
# Cargo.lock and a CHANGELOG.md section listing every commit since the last
# `v*` tag (cox commits are `<task-id>: <title>`, so there are no groups). This
# script never pushes and never tags: bump.yml lands the commit through a pull
# request with green required checks, then tags the commit that landed on main.
set -euo pipefail

cd "$(dirname "$0")/.."

die() { echo "error: $*" >&2; exit 1; }

level="${1:-patch}"
mode="${2:-}"
case "$level" in
  patch | minor | major) ;;
  *) echo "level must be patch, minor or major (got '$level')" >&2; exit 2 ;;
esac
case "$mode" in
  "")
    # The only way to a release: the Bump workflow (PR, checks, merge, tag).
    gh workflow run bump.yml -f level="$level"
    echo "Bump and release ($level) started: gh run list --workflow bump.yml"
    exit 0 ;;
  --dry-run | --local) ;;
  *) echo "unknown option: $mode" >&2; exit 2 ;;
esac

CARGO="${CARGO:-cargo}"

workspace_version() {
  awk '/^\[workspace\.package\]/ { on = 1; next }
       /^\[/ { on = 0 }
       on && /^version[[:space:]]*=/ { split($0, q, "\""); print q[2]; exit }' Cargo.toml
}

current="$(workspace_version)"
[ -n "$current" ] || die "could not read [workspace.package] version from Cargo.toml"

version="$current"
if git rev-parse -q --verify "refs/tags/v$current" >/dev/null; then
  IFS=. read -r major minor patch <<<"${current%%-*}"
  case "$level" in
    major) version="$((major + 1)).0.0" ;;
    minor) version="$major.$((minor + 1)).0" ;;
    patch) version="$major.$minor.$((patch + 1))" ;;
  esac
fi

echo "current $current -> release v$version"
# bump.yml reads this to know which tag to make.
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "version=$version" >>"$GITHUB_OUTPUT"
fi

if [ "$mode" = "--dry-run" ]; then
  echo "dry run: nothing written"
  exit 0
fi

if [ "$version" = "$current" ]; then
  echo "local: v$version is not tagged yet; nothing to commit"
  exit 0
fi

[ -z "$(git status --porcelain)" ] || die "working tree is not clean; commit or stash first"

prev="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
range="${prev:+$prev..}HEAD"
if [ -n "$prev" ]; then
  link="https://github.com/pyrlyn/cox/compare/$prev...v$version"
else
  link="https://github.com/pyrlyn/cox/releases/tag/v$version"
fi
git log --no-merges --format=%s "$range" | grep -qv '^chore: release v' \
  || die "no commits since ${prev:-the start} to release"

awk -v new="$version" '
  /^\[workspace\.package\]/ { on = 1; print; next }
  /^\[/ { on = 0 }
  on && /^version[[:space:]]*=/ && !done { printf "version = \"%s\"\n", new; done = 1; next }
  { print }
' Cargo.toml >Cargo.toml.new && mv Cargo.toml.new Cargo.toml
[ "$(workspace_version)" = "$version" ] || die "Cargo.toml did not take version $version"
# Cargo.lock records the members' own versions too; --locked builds fail if
# the two disagree.
$CARGO update --workspace --quiet

entry="$(mktemp)"
trap 'rm -f "$entry" CHANGELOG.md.new' EXIT
{
  echo "## [$version]($link) - $(date -u +%Y-%m-%d)"
  echo
  git log --reverse --no-merges --abbrev=7 --format='* %s (%h)' "$range" \
    | grep -v '^\* chore: release v' || true
  echo
} >"$entry"
# The new section goes above the previous release, under the header.
awk -v entry="$entry" '
  !done && /^## / { while ((getline line < entry) > 0) print line; done = 1 }
  { print }
  END { if (!done) while ((getline line < entry) > 0) print line }
' CHANGELOG.md >CHANGELOG.md.new && mv CHANGELOG.md.new CHANGELOG.md

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit --quiet -m "chore: release v$version"
echo "local: version commit made, not pushed, not tagged"
