#!/usr/bin/env bash
# sepia release — thin shim over tarsi-serve (the org release plane,
# project ADR-0005; repo config: ./tarsi.toml). One call runs
# plan → changelog → bump → tag → push; publish targets are off
# (ADR-0004: crates.io is tamias-only — the `sepia` name on
# crates.io is taken anyway — and GitHub Releases are unused).
# Operator-run on main — `tarsi-serve release` is dry-run by
# default; --yes is the explicit opt-in.
#
# crates.io publish is GONE: the old per-crate `cargo publish`
# loop (and its crates.io ordering constraints — hypha → sepia →
# peregrin) is retired; every sepia crate is an internal lib-only
# engine behind `tamias-driver-sepia`, and the `sepia` command
# ships as the tamias pack in `libraries/sepia`.
# `sepia-testkit`, `sepia-driver-config`, `sepia-driver-pavo` are
# publish = false.
#
# Pack-ref sync (`[release.sync_refs]` in tarsi.toml) rewrites the
# tamias pack manifest + README install ref inside the release
# commit, so the tagged tree carries consistent versions. The pack
# publish itself stays manual (`tamias publish libraries/sepia`).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# worktree-local env (gitignored); the service env file is canonical
[ -f ./.env ] && { set -a; source ./.env; set +a; }

tarsi-serve release --dir . --yes

echo "released: see tags sepia-*-v<ver>"
echo "next: cargo xtask pack && tamias publish libraries/sepia  # ship 9aia:sepia@<ver> to the org registry"
