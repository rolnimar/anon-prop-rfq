#!/usr/bin/env python3
"""Check artifact outputs against the paper's quantitative claims."""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
from typing import Any


class Checks:
    def __init__(self) -> None:
        self.total = 0
        self.failures: list[str] = []

    def equal(self, label: str, actual: Any, expected: Any) -> None:
        self.total += 1
        if actual == expected:
            print(f"[ok] {label}: {actual}")
            return
        failure = f"{label}: expected {expected!r}, found {actual!r}"
        self.failures.append(failure)
        print(f"[fail] {failure}")

    def true(self, label: str, value: bool) -> None:
        self.equal(label, value, True)

    def finish(self) -> None:
        if self.failures:
            raise SystemExit(
                f"{len(self.failures)} of {self.total} checks failed:\n"
                + "\n".join(f"- {failure}" for failure in self.failures)
            )
        print(f"all {self.total} paper-claim checks passed")


def csv_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def by(rows: list[dict[str, str]], **fields: str) -> dict[str, str]:
    matches = [
        row
        for row in rows
        if all(row.get(key) == value for key, value in fields.items())
    ]
    if len(matches) != 1:
        raise RuntimeError(f"expected one row for {fields}, found {len(matches)}")
    return matches[0]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--expected-resource-samples", type=int, default=100)
    args = parser.parse_args()
    root = args.results
    checks = Checks()

    contract = json.loads((root / "evaluation_contract.json").read_text())
    grid = contract["parameter_grid"]
    checks.equal("evaluation protocol revision", contract["protocol_revision"], 9)
    checks.equal("parameter Cartesian product", grid["cartesian_product_count"], 15_120)
    checks.equal(
        "parameter dimension lengths",
        [
            len(grid["haircut_bps"]),
            len(grid["exponent_scaled_1e4"]),
            len(grid["wall_sensitivity_scaled_1e4"]),
            len(grid["cadence_threshold"]),
            len(grid["cadence_wave_scaled_1e4"]),
        ],
        [10, 9, 7, 4, 6],
    )
    checks.equal(
        "aggregate split workloads",
        contract["workloads"]["splitting"]["aggregate_mitigation_workloads"],
        40,
    )

    baseline = {row["mechanism"]: row for row in csv_rows(root / "baseline_summary.csv")}
    expected_baselines = {
        "fixed_nav": (0, 0, 1_000),
        "static_700_bps": (700, 0, 1_630),
        "integrated_curve": (539, 0, 1_138),
        "prop_endpoint": (1, 537, 1_000),
        "prop_pressure": (1, 7_001, 1_049),
        "prop_full": (7, 6_205, 1_766),
    }
    for mechanism, expected in expected_baselines.items():
        row = baseline[mechanism]
        actual = (
            int(row["worst_protected_order_discount_bps"]),
            int(row["max_split_gain_nav_bps"]),
            int(row["minimum_reserve_remaining_bps"]),
        )
        checks.equal(f"baseline {mechanism}", actual, expected)

    sweep = csv_rows(root / "parameter_sweep.csv")
    checks.equal("parameter sweep rows", len(sweep), 15_120)
    checks.equal(
        "simple-baseline dominated rows",
        sum(row["dominating_simple_baseline"] != "none" for row in sweep),
        541,
    )
    checks.equal(
        "trade-off eligible rows",
        sum(row["tradeoff_eligible"] == "true" for row in sweep),
        14_579,
    )
    checks.equal(
        "strict grief diagnostic passes",
        sum(row["security_eligible"] == "true" for row in sweep),
        2_484,
    )
    checks.true(
        "all grief-passing rows disable cadence",
        all(
            row["cadence_wave_scaled"] == "0"
            for row in sweep
            if row["security_eligible"] == "true"
        ),
    )
    checks.true(
        "every nonzero-cadence row fails strict grief diagnostic",
        all(
            row["grief_pass"] == "false"
            for row in sweep
            if row["cadence_wave_scaled"] != "0"
        ),
    )
    checks.equal(
        "non-dominated frontier rows",
        len(csv_rows(root / "parameter_frontier.csv")),
        1_651,
    )

    reference = by(sweep, parameter_id="h700_e25000_s20000_k20_y10000")
    checks.equal(
        "diagnostic cadence metrics",
        (
            int(reference["aggregate_split_mitigation_bps"]),
            int(reference["improved_split_workloads"]),
            int(reference["unchanged_split_workloads"]),
            int(reference["worsened_split_workloads"]),
        ),
        (967, 34, 6, 0),
    )
    checks.equal(
        "diagnostic full metrics",
        (
            int(reference["worst_small_discount_bps"]),
            int(reference["max_split_gain_nav_bps"]),
            int(reference["max_split_gain_relative_bps"]),
            int(reference["minimum_reserve_bps"]),
            int(reference["worst_honest_burst_discount_bps"]),
            int(reference["maximum_funded_roundtrip_profit"]),
            int(reference["recovery_parity_max_abs"]),
        ),
        (7, 6_205, 21_081, 1_766, 36, -553_032, 0),
    )
    pressure_reference = by(sweep, parameter_id="h700_e25000_s20000_k20_y0")
    checks.equal(
        "pressure-only honest-burst discount bps",
        int(pressure_reference["worst_honest_burst_discount_bps"]),
        1,
    )
    strongest = by(sweep, parameter_id="h100_e100000_s5000_k5_y50000")
    checks.equal(
        "strongest mitigation extreme",
        (
            int(strongest["aggregate_split_mitigation_bps"]),
            int(strongest["minimum_reserve_bps"]),
            int(strongest["worst_small_discount_bps"]),
            int(strongest["improved_split_workloads"]),
            int(strongest["unchanged_split_workloads"]),
        ),
        (10_000, 3_023, 317, 6, 34),
    )

    splitting = csv_rows(root / "splitting.csv")
    for mechanism, timing, expected in [
        ("prop_endpoint", "within_epoch", 125),
        ("prop_pressure", "within_epoch", 751),
        ("prop_full", "within_epoch", 73),
        ("prop_full", "boundary", 453),
    ]:
        row = by(
            splitting,
            mechanism=mechanism,
            total_exit_bps="5000",
            parts="20",
            timing=timing,
        )
        checks.equal(
            f"50% exit split gain {mechanism} {timing}",
            int(row["split_gain_bps"]),
            expected,
        )
    row = by(
        splitting,
        mechanism="prop_full",
        total_exit_bps="9000",
        parts="20",
        timing="within_epoch",
    )
    checks.equal("90% exit relative split gain bps", int(row["split_gain_bps"]), 19_619)

    cadence = csv_rows(root / "cadence.csv")
    dust = by(
        cadence,
        mechanism="prop_full",
        traffic="dust_grief",
        preliminary_trade_count="20",
    )
    checks.equal(
        "20-dust-sale grief trace",
        (
            int(dust["preliminary_loss"]),
            int(dust["victim_harm"]),
            int(dust["cost_harm_ratio_bps"]),
        ),
        (19, 2_653, 71),
    )

    buy_relief = csv_rows(root / "buy_relief.csv")
    funded_nonzero = [
        row
        for row in buy_relief
        if row["mechanism"] == "prop_full"
        and row["mode"] == "funded_refill"
        and row["relief_bps_of_stress"] != "0"
    ]
    checks.equal(
        "closest nonzero funded round-trip result",
        max(int(row["attacker_profit"]) for row in funded_nonzero),
        -553_032,
    )

    ordering = csv_rows(root / "ordering.csv")
    checks.equal(
        "protected-order payouts by ordering",
        (
            int(by(ordering, mechanism="prop_full", ordering="whale_first")["small_payout"]),
            int(by(ordering, mechanism="prop_full", ordering="small_first")["small_payout"]),
        ),
        (999_309, 999_999),
    )

    recovery = csv_rows(root / "recovery.csv")
    checks.equal(
        "maximum implementation-to-rolled-model recovery gap",
        max(abs(int(row["implementation_minus_model"])) for row in recovery),
        0,
    )

    oracle = json.loads((root / "oracle_verification.json").read_text())
    checks.equal("independent pricing vectors", oracle["pricing_vectors"], 34)
    checks.equal("pricing fixture rows", oracle["pricing_fixture_rows"], 34)
    checks.equal(
        "maximum independent-oracle gap",
        oracle["maximum_absolute_difference_base_units"],
        20,
    )
    checks.equal("decimal conversion vectors", oracle["decimal_vectors"], 5)
    checks.equal("oracle test groups passed", oracle["test_groups_passed"], 2)

    profile = {row["instruction"]: row for row in csv_rows(root / "resource_profile.csv")}
    checks.true(
        "resource instructions present",
        set(profile) == {"quote_sell", "open_swap_sell", "quote_swap_buy", "open_swap_buy"},
    )
    checks.true(
        "resource sample count",
        all(int(row["samples"]) == args.expected_resource_samples for row in profile.values()),
    )
    checks.equal(
        "deterministic compute-unit maxima",
        (
            int(profile["quote_sell"]["compute_units_max"]),
            int(profile["open_swap_sell"]["compute_units_max"]),
            int(profile["quote_swap_buy"]["compute_units_max"]),
            int(profile["open_swap_buy"]["compute_units_max"]),
        ),
        (60_578, 247_320, 27_277, 205_828),
    )
    checks.equal(
        "version-0 sell and buy transaction bytes",
        (
            int(profile["open_swap_sell"]["v0_lookup_wire_bytes"]),
            int(profile["open_swap_buy"]["v0_lookup_wire_bytes"]),
        ),
        (424, 430),
    )
    accounts = csv_rows(root / "resource_accounts.csv")
    pair_state = by(accounts, account="prop_rfq_pair_state")
    checks.equal(
        "pair-state bytes and rent",
        (int(pair_state["data_bytes"]), int(pair_state["rent_exempt_lamports"])),
        (460, 4_092_480),
    )
    profile_metadata = json.loads((root / "resource_profile_metadata.json").read_text())
    checks.equal(
        "resource metadata samples",
        profile_metadata["samples"],
        args.expected_resource_samples,
    )
    checks.equal(
        "deterministic resource seed scheme",
        profile_metadata["resource_profile_seed"],
        "PROP_RFQ_RESOURCE_SAMPLE=0..samples-1",
    )

    metadata = json.loads((root / "metadata.json").read_text())
    checks.equal("scenario randomness", metadata["randomness"], "none")
    checks.equal("initial liquidity", metadata["initial_liquidity"], 1_000_000_000)
    checks.equal("diagnostic haircut", metadata["curve_peg_haircut_bps"], 700)
    checks.equal("diagnostic exponent", metadata["curve_exponent_scaled"], 25_000)
    checks.equal("diagnostic wall sensitivity", metadata["wall_sensitivity_scaled"], 20_000)
    checks.equal("diagnostic cadence threshold", metadata["cadence_threshold"], 20)
    checks.equal("diagnostic cadence wave", metadata["cadence_wave_scaled"], 10_000)
    checks.equal("diagnostic epoch seconds", metadata["epoch_duration_seconds"], 86_400)
    checks.equal("source worktree clean during scenario run", metadata["working_tree_dirty"], False)

    checks.finish()


if __name__ == "__main__":
    main()
