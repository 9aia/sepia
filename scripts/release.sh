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
# Preserved repo step: the tamias pack manifest + README install
# ref are synced to the new version *before* the release commit
# lands, so the tagged tree carries consistent versions. The pack
# publish itself stays manual (`tamias publish libraries/sepia`).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# worktree-local env (gitignored); the service env file is canonical
[ -f ./.env ] && { set -a; source ./.env; set +a; }

ws_version() { grep -m1 '^version = "' Cargo.toml | cut -d'"' -f2; }

# Learn the version tarsi will bump to (all releasing units share one
# workspace version — empty output means "don't sync").
new="$(tarsi-serve plan --dir . | python3 -c '
import json,sys
d=json.load(sys.stdin)
if not d.get("ok"): sys.exit(0)
units=[u for u in d["result"]["plan"]["units"] if u.get("releases")]
vs={u["new_version"] for u in units}
print(vs.pop() if len(vs)==1 else "")
')"
old="$(ws_version)"
if [ -n "$new" ] && [ "$new" != "$old" ]; then
    sed -i "s/^version: \"${old}\"/version: \"${new}\"/" libraries/sepia/tamias.yaml
    sed -i "s|sepia@${old}|sepia@${new}|g" README.md
    git add libraries/sepia/tamias.yaml README.md
    git commit -qm "chore(release): sync sepia@${new} version refs ahead of tarsi bump"
fi

tarsi-serve release --dir . --yes

echo "released: sepia ${new:-$old} (tag sepia-v${new:-$old}, per-unit tags for the rest)"
echo "next: cargo xtask pack && tamias publish libraries/sepia  # ship 9aia:sepia@${new:-$old} to the org registry"
