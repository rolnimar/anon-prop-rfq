#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: prepare-anchor-sbpf-toolchain.sh [--cleanup-existing]

Inspect registered Solana SBF toolchains without removing them by default.
--cleanup-existing explicitly uninstalls matching global rustup registrations.
EOF
}

if (( $# > 1 )); then
    printf 'Unexpected arguments.\n' >&2
    usage >&2
    exit 2
fi

cleanup_existing=false
if (( $# == 1 )); then
    case "$1" in
        --cleanup-existing) cleanup_existing=true ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
fi

# Anchor 1.1.2 can misparse rustup's verbose output for an existing linked SBF
# toolchain. The platform tools recreate this generated link during the build.
registered_toolchains="$(rustup toolchain list)"
matching_toolchains=()
while IFS= read -r line; do
    toolchain="${line%% *}"
    case "$toolchain" in
        *-sbpf-solana-v*) matching_toolchains+=("$toolchain") ;;
    esac
done <<< "$registered_toolchains"

if (( ${#matching_toolchains[@]} == 0 )); then
    exit 0
fi

printf 'Existing Solana SBF toolchain registrations:\n' >&2
printf '  %s\n' "${matching_toolchains[@]}" >&2
if [[ "$cleanup_existing" != true ]]; then
    printf '%s\n' \
        'Preparation stopped to avoid an Anchor 1.1.2 toolchain parsing bug.' \
        'No toolchains were removed. To explicitly remove these registrations, run:' \
        '  bash scripts/prepare-anchor-sbpf-toolchain.sh --cleanup-existing' \
        'Then retry the build.' >&2
    exit 1
fi

for toolchain in "${matching_toolchains[@]}"; do
    rustup toolchain uninstall -- "$toolchain"
done
