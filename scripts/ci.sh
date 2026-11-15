#!/usr/bin/env bash
# ci-gate gate for main — runs in a detached worktree of the pushed
# sha via bupha. Rust floor + org docs contract.
set -euo pipefail

# 16 cores / ~11 GiB RAM — cargo jobs OOM under earlyoom on this host.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

cargo xtask check
cargo xtask test

# Docs contract — cypera check + docs/ drift gate (hermetic: materializes
# a detached docs-branch worktree when the run tree lacks one).
bash scripts/docs-check.sh
