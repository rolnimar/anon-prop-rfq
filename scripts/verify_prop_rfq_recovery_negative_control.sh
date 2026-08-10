#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
  echo "negative control requires a clean tracked worktree" >&2
  exit 1
fi

negative_dir="$(mktemp -d /tmp/prop-rfq-recovery-negative.XXXXXX)"
cleanup() {
  git -C "$repo_root" worktree remove --force "$negative_dir" >/dev/null 2>&1 || true
  git -C "$repo_root" worktree prune >/dev/null 2>&1 || true
}
trap cleanup EXIT

git -C "$repo_root" worktree add --detach "$negative_dir" HEAD
git -C "$negative_dir" apply scripts/prop_rfq_recovery_negative_control.patch

(
  cd "$negative_dir"
  bash scripts/prepare-anchor-sbpf-toolchain.sh
  anchor build
)

set +e
test_output="$(
  cd "$negative_dir"
  cargo test \
    --manifest-path programs/rwa_exit/Cargo.toml \
    --test prop_rfq \
    test_sbf_quotes_match_independent_oracle_vectors \
    -- --nocapture 2>&1
)"
test_status=$?
set -e

if [[ $test_status -eq 0 ]]; then
  echo "negative control unexpectedly passed" >&2
  exit 1
fi

failure_line="$(
  printf '%s\n' "$test_output" |
    grep 'recovery_second_epoch_half: SBF payout' |
    head -n 1
)"
if [[ -z "$failure_line" ]]; then
  printf '%s\n' "$test_output" >&2
  echo "negative control failed for an unexpected reason" >&2
  exit 1
fi

echo "negative control reproduced: $failure_line"
