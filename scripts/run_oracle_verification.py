#!/usr/bin/env python3
"""Run rebuilt-SBF oracle tests and write a compact machine-readable result."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


SUMMARY = re.compile(
    r"independent oracle vectors: (?P<count>\d+), "
    r"maximum absolute difference: (?P<gap>\d+) base units, "
    r"worst vector: (?P<worst>[A-Za-z0-9_]+)"
)


def data_rows(path: Path) -> int:
    with path.open(encoding="utf-8") as handle:
        return sum(1 for line in handle if line.strip()) - 1


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    repo = Path(__file__).resolve().parents[1]
    result = subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            "programs/rwa_exit/Cargo.toml",
            "--test",
            "prop_rfq",
            "independent_",
            "--",
            "--nocapture",
        ],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    )
    output = result.stdout + result.stderr
    match = SUMMARY.search(output)
    if match is None:
        raise RuntimeError("oracle test output did not contain the expected summary")
    if "test result: ok. 2 passed" not in output:
        raise RuntimeError("both oracle test groups did not pass")

    fixture_dir = repo / "programs/rwa_exit/tests/fixtures"
    payload = {
        "pricing_vectors": int(match.group("count")),
        "maximum_absolute_difference_base_units": int(match.group("gap")),
        "worst_vector": match.group("worst"),
        "decimal_vectors": data_rows(fixture_dir / "prop_rfq_decimal_vectors.csv"),
        "pricing_fixture_rows": data_rows(fixture_dir / "prop_rfq_oracle_vectors.csv"),
        "test_groups_passed": 2,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()

