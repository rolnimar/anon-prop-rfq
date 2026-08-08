#!/usr/bin/env python3
"""Independent high-precision reference model for Prop RFQ sell quotes.

This file intentionally does not import the Rust program, its host evaluation
harness, or generated program interfaces. It implements the documented integer
state transitions directly and uses Decimal arithmetic for the fractional
endpoint power.
"""

from __future__ import annotations

import argparse
import csv
import io
from dataclasses import dataclass, fields
from decimal import Decimal, ROUND_FLOOR, getcontext
from pathlib import Path
from typing import Iterable

getcontext().prec = 100

HARD_WALL_SCALE = 1_000_000_000_000
CONFIG_SCALE = 10_000
ACTUAL_LIQUIDITY = 10_000_000_000


@dataclass(frozen=True)
class OracleVector:
    name: str
    raw_value: int
    actual_liquidity: int = ACTUAL_LIQUIDITY
    haircut_bps: int = 700
    exponent_scaled: int = 25_000
    wall_sensitivity_scaled: int = 20_000
    cadence_threshold: int = 20
    cadence_wave_scaled: int = 10_000
    epoch_duration: int = 86_400
    elapsed: int = 0
    curr_sell: int = 0
    curr_buy: int = 0
    prev_net_sell: int = 0
    sell_count: int = 0
    expected_payout: int = 0
    tolerance: int = 0


@dataclass(frozen=True)
class DecimalVector:
    name: str
    token_input: int
    price_scaled: int
    token_in_decimals: int
    token_out_decimals: int
    expected_raw_value: int = 0


def floor_decimal(value: Decimal) -> int:
    return int(value.to_integral_value(rounding=ROUND_FLOOR))


def preview_pressure(vector: OracleVector) -> int:
    current_net = max(vector.curr_sell - vector.curr_buy, 0)
    if vector.elapsed < 0 or vector.elapsed >= 2 * vector.epoch_duration:
        previous = 0
        current = 0
        elapsed_in_epoch = 0
    elif vector.elapsed >= vector.epoch_duration:
        previous = current_net
        current = 0
        elapsed_in_epoch = vector.elapsed - vector.epoch_duration
    else:
        previous = vector.prev_net_sell
        current = current_net
        elapsed_in_epoch = vector.elapsed

    remaining = vector.epoch_duration - elapsed_in_epoch
    decayed_previous = previous * remaining // vector.epoch_duration
    return decayed_previous + current + vector.raw_value


def preview_sell_count(vector: OracleVector) -> int:
    if vector.elapsed < 0 or vector.elapsed >= vector.epoch_duration:
        return 0
    return vector.sell_count


def exact_endpoint_power_scaled(utilization_scaled: int, exponent_scaled: int) -> int:
    if utilization_scaled == 0:
        return 0
    utilization = Decimal(utilization_scaled) / Decimal(HARD_WALL_SCALE)
    exponent = Decimal(exponent_scaled) / Decimal(CONFIG_SCALE)
    value = (utilization.ln() * exponent).exp()
    return floor_decimal(value * Decimal(HARD_WALL_SCALE))


def oracle_payout(vector: OracleVector) -> int:
    if not 0 < vector.raw_value <= vector.actual_liquidity:
        raise ValueError(f"{vector.name}: raw value must be within funded liquidity")
    if vector.actual_liquidity <= 0:
        raise ValueError(f"{vector.name}: funded liquidity must be positive")
    if vector.wall_sensitivity_scaled < 0:
        raise ValueError(f"{vector.name}: wall sensitivity must be nonnegative")
    if vector.cadence_threshold <= 0 or vector.epoch_duration <= 0:
        raise ValueError(f"{vector.name}: cadence threshold and epoch must be positive")

    pressure = preview_pressure(vector)
    sensitivity_component = (
        vector.wall_sensitivity_scaled * pressure // vector.actual_liquidity
    )
    dynamic_wall = (
        vector.actual_liquidity
        * CONFIG_SCALE
        // (CONFIG_SCALE + sensitivity_component)
    )
    effective_liquidity = max(dynamic_wall, 1)
    utilization_scaled = (
        vector.raw_value * HARD_WALL_SCALE // effective_liquidity
    )

    endpoint_power = exact_endpoint_power_scaled(
        utilization_scaled, vector.exponent_scaled
    )
    peg_haircut_scaled = (
        HARD_WALL_SCALE * vector.haircut_bps // CONFIG_SCALE
    )
    endpoint_haircut = (
        peg_haircut_scaled * endpoint_power // HARD_WALL_SCALE
    )

    count = preview_sell_count(vector)
    if count >= vector.cadence_threshold:
        cadence_ramp = CONFIG_SCALE
    else:
        cadence_ramp = count * CONFIG_SCALE // vector.cadence_threshold
    wave_height = (
        vector.cadence_wave_scaled * cadence_ramp // CONFIG_SCALE
    )

    normalized = min(utilization_scaled, HARD_WALL_SCALE)
    if normalized == 0 or wave_height == 0:
        cadence_haircut = 0
    else:
        remaining = HARD_WALL_SCALE - normalized
        eased_numerator = normalized * 8
        eased_denominator = eased_numerator + remaining
        eased_rise = eased_numerator * HARD_WALL_SCALE // eased_denominator
        cadence_haircut = min(
            HARD_WALL_SCALE,
            eased_rise * wave_height // (CONFIG_SCALE * 3),
        )

    haircut = max(endpoint_haircut, cadence_haircut)
    payout_factor = max(HARD_WALL_SCALE - haircut, 0)
    return vector.raw_value * payout_factor // HARD_WALL_SCALE


def tolerance_for(raw_value: int) -> int:
    return max(3, (raw_value + 49_999_999) // 50_000_000)


def raw_value_from_nav(vector: DecimalVector) -> int:
    numerator = (
        vector.token_input
        * vector.price_scaled
        * 10**vector.token_out_decimals
    )
    denominator = 10 ** (vector.token_in_decimals + 9)
    return numerator // denominator


def decimal_vectors() -> list[DecimalVector]:
    return [
        DecimalVector(
            name=f"asset_decimals_{decimals}",
            token_input=1_234_567_890,
            price_scaled=1_000_000_000,
            token_in_decimals=9,
            token_out_decimals=decimals,
            expected_raw_value=raw_value_from_nav(
                DecimalVector(
                    name="",
                    token_input=1_234_567_890,
                    price_scaled=1_000_000_000,
                    token_in_decimals=9,
                    token_out_decimals=decimals,
                )
            ),
        )
        for decimals in [0, 2, 6, 8, 9]
    ]


def base_vectors() -> list[OracleVector]:
    vectors = [
        OracleVector("reference_raw_1", 1),
        OracleVector("reference_raw_1000", 1_000),
        OracleVector("reference_raw_1m", 1_000_000),
        OracleVector("reference_raw_100m", 100_000_000),
        OracleVector("reference_raw_1b", 1_000_000_000),
        OracleVector("reference_raw_5b", 5_000_000_000),
        OracleVector("reference_raw_9b", 9_000_000_000),
        OracleVector("reference_raw_vault", 10_000_000_000),
        OracleVector("liquidity_one_unit", 1, actual_liquidity=1),
        OracleVector("liquidity_100_units", 99, actual_liquidity=100),
        OracleVector(
            "pressure_count_1",
            100_000_000,
            curr_sell=2_000_000_000,
            sell_count=1,
        ),
        OracleVector(
            "pressure_count_5",
            100_000_000,
            curr_sell=2_000_000_000,
            sell_count=5,
        ),
        OracleVector(
            "pressure_count_19",
            100_000_000,
            curr_sell=2_000_000_000,
            sell_count=19,
        ),
        OracleVector(
            "pressure_count_20",
            100_000_000,
            curr_sell=2_000_000_000,
            sell_count=20,
        ),
        OracleVector(
            "first_epoch_half",
            250_000_000,
            elapsed=43_200,
            curr_sell=5_000_000_000,
            curr_buy=1_000_000_000,
            prev_net_sell=3_000_000_000,
            sell_count=10,
        ),
        OracleVector(
            "first_epoch_last_second",
            250_000_000,
            elapsed=86_399,
            curr_sell=5_000_000_000,
            curr_buy=1_000_000_000,
            prev_net_sell=3_000_000_000,
            sell_count=10,
        ),
        OracleVector(
            "recovery_epoch_boundary",
            250_000_000,
            elapsed=86_400,
            curr_sell=5_000_000_000,
            curr_buy=1_000_000_000,
            prev_net_sell=3_000_000_000,
            sell_count=10,
        ),
        OracleVector(
            "recovery_second_epoch_half",
            250_000_000,
            elapsed=129_600,
            curr_sell=5_000_000_000,
            curr_buy=1_000_000_000,
            prev_net_sell=3_000_000_000,
            sell_count=10,
        ),
        OracleVector(
            "recovery_two_epochs",
            250_000_000,
            elapsed=172_800,
            curr_sell=5_000_000_000,
            curr_buy=1_000_000_000,
            prev_net_sell=3_000_000_000,
            sell_count=10,
        ),
        OracleVector(
            "backward_timestamp_reset",
            250_000_000,
            elapsed=-1,
            curr_sell=5_000_000_000,
            curr_buy=1_000_000_000,
            prev_net_sell=3_000_000_000,
            sell_count=10,
        ),
        OracleVector("exponent_0_1", 1_000_000_000, exponent_scaled=1_000),
        OracleVector("exponent_1", 1_000_000_000, exponent_scaled=10_000),
        OracleVector("exponent_5", 1_000_000_000, exponent_scaled=50_000),
        OracleVector("exponent_10", 1_000_000_000, exponent_scaled=100_000),
        OracleVector("zero_peg_haircut", 5_000_000_000, haircut_bps=0),
        OracleVector("full_peg_haircut", 5_000_000_000, haircut_bps=10_000),
        OracleVector(
            "minimum_wall_sensitivity",
            1_000_000_000,
            wall_sensitivity_scaled=1,
        ),
        OracleVector(
            "high_wall_sensitivity",
            1_000_000_000,
            wall_sensitivity_scaled=100_000,
        ),
        OracleVector(
            "cadence_disabled",
            100_000_000,
            cadence_wave_scaled=0,
            curr_sell=2_000_000_000,
            sell_count=19,
        ),
        OracleVector(
            "cadence_max_at_one",
            100_000_000,
            cadence_threshold=1,
            cadence_wave_scaled=50_000,
            curr_sell=2_000_000_000,
            sell_count=1,
        ),
        OracleVector(
            "cadence_max_near_threshold",
            100_000_000,
            cadence_threshold=40,
            cadence_wave_scaled=50_000,
            curr_sell=2_000_000_000,
            sell_count=39,
        ),
        OracleVector(
            "buy_relief_saturates_at_zero",
            100_000_000,
            curr_sell=1_000_000_000,
            curr_buy=2_000_000_000,
            sell_count=5,
        ),
        OracleVector(
            "previous_pressure_half_decay",
            100_000_000,
            elapsed=43_200,
            prev_net_sell=8_000_000_000,
        ),
        OracleVector(
            "harness_endpoint_ablation",
            1_000_000_000,
            wall_sensitivity_scaled=0,
            cadence_wave_scaled=0,
        ),
    ]
    return [
        OracleVector(
            **{
                **{
                    field.name: getattr(vector, field.name)
                    for field in fields(OracleVector)
                },
                "expected_payout": oracle_payout(vector),
                "tolerance": tolerance_for(vector.raw_value),
            }
        )
        for vector in vectors
    ]


def render_dataclass_rows(vectors: Iterable[object], row_type: type) -> str:
    output = io.StringIO(newline="")
    try:
        writer = csv.DictWriter(
            output,
            fieldnames=[field.name for field in fields(row_type)],
            lineterminator="\n",
        )
        writer.writeheader()
        for vector in vectors:
            writer.writerow(
                {
                    field.name: getattr(vector, field.name)
                    for field in fields(row_type)
                }
            )
        return output.getvalue()
    finally:
        output.close()


def write_vectors(path: Path, vectors: Iterable[OracleVector]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        render_dataclass_rows(vectors, OracleVector), encoding="utf-8"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(
            "programs/rwa_exit/tests/fixtures/prop_rfq_oracle_vectors.csv"
        ),
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the tracked vector file differs from fresh oracle output",
    )
    parser.add_argument(
        "--decimal-output",
        type=Path,
        default=Path(
            "programs/rwa_exit/tests/fixtures/prop_rfq_decimal_vectors.csv"
        ),
    )
    args = parser.parse_args()
    vectors = base_vectors()
    decimals = decimal_vectors()
    if args.check:
        expected = render_dataclass_rows(vectors, OracleVector)
        actual = args.output.read_text(encoding="utf-8")
        if actual != expected:
            raise SystemExit(
                f"{args.output} is stale; regenerate it with this script"
            )
        expected_decimals = render_dataclass_rows(decimals, DecimalVector)
        actual_decimals = args.decimal_output.read_text(encoding="utf-8")
        if actual_decimals != expected_decimals:
            raise SystemExit(
                f"{args.decimal_output} is stale; regenerate it with this script"
            )
        print(
            f"verified {len(vectors)} pricing vectors and {len(decimals)} decimal vectors"
        )
    else:
        write_vectors(args.output, vectors)
        args.decimal_output.parent.mkdir(parents=True, exist_ok=True)
        args.decimal_output.write_text(
            render_dataclass_rows(decimals, DecimalVector),
            encoding="utf-8",
        )
        print(
            f"wrote {len(vectors)} pricing vectors and {len(decimals)} decimal vectors"
        )


if __name__ == "__main__":
    main()
