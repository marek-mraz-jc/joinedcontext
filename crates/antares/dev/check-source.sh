#!/usr/bin/env bash
# The branch model of CONTRIBUTING.md, held mechanically by full.yml:
#
#   pull request into main — its head is this repository's dev;
#   v* tag                   — names a commit on main, and the workspace
#                              version in Cargo.toml is the tag's version.
#
# Every other event passes. Reads the GitHub Actions environment plus
# HEAD_REPO (the pull request's head repository); needs full history.
# `dev/check-source.sh --self-test` proves the rules in a scratch repository.
set -euo pipefail

check() {
  case "$GITHUB_EVENT_NAME" in
    pull_request)
      if [ "$HEAD_REPO" != "$GITHUB_REPOSITORY" ] || [ "$GITHUB_HEAD_REF" != dev ]; then
        echo "::error::a pull request into main is the promotion of dev; retarget this one to dev"
        return 1
      fi ;;
    push)
      case "$GITHUB_REF" in refs/tags/v*) ;; *) return 0 ;; esac
      local tag=${GITHUB_REF#refs/tags/v} version
      if ! git merge-base --is-ancestor "$GITHUB_SHA" origin/main; then
        echo "::error::v$tag is not on main; a release is tagged on the promotion's merge commit"
        return 1
      fi
      version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
      if [ "$version" != "$tag" ]; then
        echo "::error::tag v$tag, workspace version $version; bump Cargo.toml on dev before the promotion"
        return 1
      fi ;;
  esac
}

self_test() {
  local d; d=$(mktemp -d); trap "rm -rf '$d'" EXIT
  cd "$d"
  git init -q -b main . && git config user.email t@t && git config user.name t
  printf '[workspace.package]\nversion = "1.2.3"\n' > Cargo.toml
  git add . && git commit -qm one
  git update-ref refs/remotes/origin/main HEAD
  local on_master; on_master=$(git rev-parse HEAD)
  git commit -q --allow-empty -m "only on dev"
  local off_master; off_master=$(git rev-parse HEAD)

  expect() { # pass|fail event head_ref head_repo ref sha
    local want=$1 got=pass
    GITHUB_EVENT_NAME=$2 GITHUB_HEAD_REF=$3 HEAD_REPO=$4 GITHUB_REPOSITORY=o/r \
      GITHUB_REF=$5 GITHUB_SHA=$6 check >/dev/null || got=fail
    [ "$got" = "$want" ] || { echo "self-test: $* gave $got"; exit 1; }
  }
  expect pass pull_request dev   o/r    refs/pull/1/merge "$on_master"
  expect fail pull_request topic o/r    refs/pull/1/merge "$on_master"
  expect fail pull_request dev   fork/r refs/pull/1/merge "$on_master"  # a fork's branch named dev
  expect pass push "" "" refs/tags/v1.2.3 "$on_master"
  expect fail push "" "" refs/tags/v1.2.3 "$off_master"                 # tagged on dev
  expect fail push "" "" refs/tags/v9.9.9 "$on_master"                  # Cargo.toml not bumped
  expect pass schedule "" "" refs/heads/dev "$off_master"
  expect pass workflow_dispatch "" "" refs/heads/dev "$off_master"
  echo "self-test: ok"
}

if [ "${1:-}" = --self-test ]; then self_test; else check; fi
