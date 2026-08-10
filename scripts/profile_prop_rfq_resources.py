#!/usr/bin/env python3
"""Measure rebuilt-SBF Prop RFQ compute and transaction resources."""

from __future__ import annotations

import argparse
import csv
import json
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path


PROFILE_PREFIX = "RESOURCE_PROFILE,"
ACCOUNT_PREFIX = "RESOURCE_ACCOUNT,"
EXPECTED_INSTRUCTIONS = {
    "quote_sell",
    "open_swap_sell",
    "quote_swap_buy",
    "open_swap_buy",
}


def run(
    command: list[str], cwd: Path, extra_env: dict[str, str] | None = None
) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        check=True,
        text=True,
        capture_output=True,
        env={
            **os.environ,
            "CARGO_TERM_COLOR": "never",
            **(extra_env or {}),
        },
    )
    return result.stdout + result.stderr


def version(command: list[str], cwd: Path) -> str:
    return run(command, cwd).strip()


def parse_bool(value: str) -> bool:
    if value == "true":
        return True
    if value == "false":
        return False
    raise ValueError(f"invalid boolean: {value}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("evaluation-output/prop-rfq"),
    )
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")

    repo = Path(__file__).resolve().parents[1]
    output = args.output if args.output.is_absolute() else repo / args.output
    output.mkdir(parents=True, exist_ok=True)

    profiles: dict[str, list[dict[str, int | bool]]] = {}
    accounts: dict[str, tuple[int, int]] = {}
    started = time.monotonic()
    for sample_index in range(args.samples):
        test_output = run(
            [
                "cargo",
                "test",
                "--manifest-path",
                "programs/rwa_exit/Cargo.toml",
                "--test",
                "prop_rfq",
                "test_dynamic_wall_ignores_buys_without_redemption_vault_refill",
                "--",
                "--nocapture",
            ],
            repo,
            {"PROP_RFQ_RESOURCE_SAMPLE": str(sample_index)},
        )
        seen = set()
        for line in test_output.splitlines():
            if line.startswith(PROFILE_PREFIX):
                (
                    _,
                    name,
                    compute_units,
                    instruction_accounts,
                    unique_legacy_accounts,
                    legacy_wire_bytes,
                    legacy_fits,
                    v0_lookup_wire_bytes,
                    v0_fits,
                ) = line.split(",")
                profiles.setdefault(name, []).append(
                    {
                        "compute_units": int(compute_units),
                        "instruction_accounts": int(instruction_accounts),
                        "unique_legacy_accounts": int(unique_legacy_accounts),
                        "legacy_wire_bytes": int(legacy_wire_bytes),
                        "legacy_fits": parse_bool(legacy_fits),
                        "v0_lookup_wire_bytes": int(v0_lookup_wire_bytes),
                        "v0_fits": parse_bool(v0_fits),
                    }
                )
                seen.add(name)
            elif line.startswith(ACCOUNT_PREFIX):
                _, name, data_bytes, rent_lamports = line.split(",")
                value = (int(data_bytes), int(rent_lamports))
                if name in accounts and accounts[name] != value:
                    raise RuntimeError(f"account measurement changed for {name}")
                accounts[name] = value
        if seen != EXPECTED_INSTRUCTIONS:
            raise RuntimeError(f"missing resource profiles: {EXPECTED_INSTRUCTIONS - seen}")

    elapsed = time.monotonic() - started
    profile_path = output / "resource_profile.csv"
    with profile_path.open("w", newline="") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=[
                "instruction",
                "samples",
                "compute_units_min",
                "compute_units_median",
                "compute_units_max",
                "instruction_accounts",
                "unique_legacy_accounts",
                "legacy_wire_bytes",
                "legacy_fits_1232_bytes",
                "v0_lookup_wire_bytes",
                "v0_lookup_fits_1232_bytes",
            ],
        )
        writer.writeheader()
        for name in sorted(profiles):
            rows = profiles[name]
            stable_fields = {
                key
                for key in rows[0]
                if key != "compute_units"
                and len({row[key] for row in rows}) == 1
            }
            expected_stable = set(rows[0]) - {"compute_units"}
            if stable_fields != expected_stable:
                raise RuntimeError(f"transaction shape changed across samples for {name}")
            compute_units = [int(row["compute_units"]) for row in rows]
            writer.writerow(
                {
                    "instruction": name,
                    "samples": len(rows),
                    "compute_units_min": min(compute_units),
                    "compute_units_median": statistics.median_low(compute_units),
                    "compute_units_max": max(compute_units),
                    "instruction_accounts": rows[0]["instruction_accounts"],
                    "unique_legacy_accounts": rows[0]["unique_legacy_accounts"],
                    "legacy_wire_bytes": rows[0]["legacy_wire_bytes"],
                    "legacy_fits_1232_bytes": rows[0]["legacy_fits"],
                    "v0_lookup_wire_bytes": rows[0]["v0_lookup_wire_bytes"],
                    "v0_lookup_fits_1232_bytes": rows[0]["v0_fits"],
                }
            )

    account_path = output / "resource_accounts.csv"
    with account_path.open("w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["account", "data_bytes", "rent_exempt_lamports"])
        for name in sorted(accounts):
            writer.writerow([name, *accounts[name]])

    metadata = {
        "samples": args.samples,
        "elapsed_seconds": round(elapsed, 3),
        "packet_data_limit_bytes": 1_232,
        "platform": {
            "system": platform.system(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "cpu_count": os.cpu_count(),
        },
        "toolchain": {
            "anchor": version(["anchor", "--version"], repo),
            "solana": version(["solana", "--version"], repo),
            "cargo_build_sbf": version(["cargo-build-sbf", "--version"], repo),
            "rustc": version(["rustc", "--version"], repo),
            "cargo": version(["cargo", "--version"], repo),
            "litesvm": "0.14.0",
            "python": platform.python_version(),
            "uv": version(["uv", "--version"], repo),
        },
        "git": {
            "commit": version(["git", "rev-parse", "HEAD"], repo),
            "dirty": bool(version(["git", "status", "--porcelain"], repo)),
        },
        "resource_profile_seed": "PROP_RFQ_RESOURCE_SAMPLE=0..samples-1",
    }
    (output / "resource_profile_metadata.json").write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n"
    )
    print(f"wrote {profile_path}")
    print(f"wrote {account_path}")
    print(f"measured {args.samples} samples in {elapsed:.3f} seconds")


if __name__ == "__main__":
    main()
