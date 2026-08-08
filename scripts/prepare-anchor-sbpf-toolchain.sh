#!/usr/bin/env bash
set -euo pipefail

# Anchor 1.1.2 can misparse rustup's verbose output for an existing linked SBF
# toolchain. The platform tools recreate this generated link during the build.
while IFS= read -r line; do
    toolchain="${line%% *}"
    case "$toolchain" in
        *-sbpf-solana-v*)
            rustup toolchain uninstall "$toolchain" >/dev/null
            ;;
    esac
done < <(rustup toolchain list)
