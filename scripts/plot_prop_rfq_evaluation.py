# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "matplotlib==3.10.7",
# ]
# ///

import argparse
import csv
from pathlib import Path

import matplotlib.pyplot as plt
from matplotlib.lines import Line2D
from matplotlib.ticker import FuncFormatter


REFERENCE_HAIRCUT_BPS = "700"
REFERENCE_EXPONENT_SCALED = "25000"
REFERENCE_WALL_SENSITIVITY_SCALED = "20000"
REFERENCE_THRESHOLD = 20
REFERENCE_WAVE = 1.0

THRESHOLD_COLORS = {
    5: "#D55E00",
    10: "#CC79A7",
    20: "#0072B2",
    40: "#009E73",
}
THRESHOLD_MARKERS = {5: "o", 10: "s", 20: "D", 40: "^"}
GRID_COLOR = "#D7DCE2"
TEXT_COLOR = "#20242A"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Plot operator-facing Prop RFQ cadence trade-offs."
    )
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--small-after-stress", type=Path, required=True)
    parser.add_argument("--output-pdf", type=Path, required=True)
    parser.add_argument("--output-svg", type=Path, required=True)
    return parser.parse_args()


def load_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def reference_curve_rows(rows: list[dict[str, str]]) -> list[dict[str, str]]:
    selected = [
        row
        for row in rows
        if row["haircut_bps"] == REFERENCE_HAIRCUT_BPS
        and row["exponent_scaled"] == REFERENCE_EXPONENT_SCALED
        and row["wall_sensitivity_scaled"]
        == REFERENCE_WALL_SENSITIVITY_SCALED
    ]
    expected = 4 * 6
    if len(selected) != expected:
        raise ValueError(
            f"expected {expected} reference-curve rows, found {len(selected)}"
        )
    return selected


def percent(value: float, _position: int) -> str:
    return f"{value:g}%"


def small_seller_payout(row: dict[str, str]) -> float:
    return 100.0 - float(row["worst_small_discount_bps"]) / 100.0


def split_benefit_removed(row: dict[str, str]) -> float:
    return float(row["aggregate_split_mitigation_bps"]) / 100.0


def liquidity_remaining(row: dict[str, str]) -> float:
    return float(row["minimum_reserve_bps"]) / 100.0


def configure_axis(axis: plt.Axes, title: str, ylabel: str) -> None:
    axis.set_title(title, loc="left", fontsize=9.0, fontweight="bold", pad=7)
    axis.set_ylabel(ylabel, fontsize=7.9)
    axis.set_xticks([0, 0.5, 1, 2, 3, 5])
    axis.set_xticklabels(["0", "0.5", "1", "2", "3", "5"])
    axis.grid(True, color=GRID_COLOR, linewidth=0.55, alpha=0.9)
    axis.set_axisbelow(True)
    axis.spines[["top", "right"]].set_visible(False)
    axis.spines[["left", "bottom"]].set_color("#69717A")
    axis.tick_params(labelsize=7.4, colors=TEXT_COLOR)
    axis.yaxis.set_major_formatter(FuncFormatter(percent))


def small_sale_rows(
    rows: list[dict[str, str]], mechanism: str
) -> list[dict[str, str]]:
    selected = [
        row
        for row in rows
        if row["mechanism"] == mechanism and row["small_bps"] == "10"
    ]
    if len(selected) != 4:
        raise ValueError(
            f"expected four 0.1% small-sale rows for {mechanism}, found {len(selected)}"
        )
    return sorted(selected, key=lambda row: int(row["stress_bps"]))


def main() -> None:
    args = parse_args()
    rows = reference_curve_rows(load_rows(args.input))
    stress_rows = load_rows(args.small_after_stress)

    plt.rcParams.update(
        {
            "font.family": "DejaVu Sans",
            "text.color": TEXT_COLOR,
            "axes.labelcolor": TEXT_COLOR,
            "pdf.fonttype": 42,
            "ps.fonttype": 42,
        }
    )

    figure, axes = plt.subplots(1, 3, figsize=(7.2, 3.8))
    pressure_small = small_sale_rows(stress_rows, "prop_pressure")
    cadence_small = small_sale_rows(stress_rows, "prop_full")
    stress_sizes = [int(row["stress_bps"]) / 100.0 for row in pressure_small]
    axes[0].plot(
        stress_sizes,
        [float(row["small_execution_bps"]) / 100.0 for row in pressure_small],
        color="#69717A",
        linestyle="--",
        marker="o",
        markersize=4,
        linewidth=1.25,
        label="Cadence off",
    )
    axes[0].plot(
        stress_sizes,
        [float(row["small_execution_bps"]) / 100.0 for row in cadence_small],
        color=THRESHOLD_COLORS[20],
        marker=THRESHOLD_MARKERS[20],
        markersize=4,
        linewidth=1.35,
        label="Reference cadence",
    )
    axes[0].set_title("(a) Price after one large sale", loc="left", fontsize=9.0, fontweight="bold", pad=7)
    axes[0].set_xlabel("Previous sale\n(% of initial vault)", fontsize=7.9)
    axes[0].set_ylabel("Next 0.1% seller receives\n(% of NAV)", fontsize=7.9)
    axes[0].set_xticks(stress_sizes)
    axes[0].xaxis.set_major_formatter(FuncFormatter(percent))
    axes[0].set_ylim(99.915, 100.005)
    axes[0].set_yticks([99.92, 99.94, 99.96, 99.98, 100.0])
    axes[0].yaxis.set_major_formatter(FuncFormatter(percent))
    axes[0].grid(True, color=GRID_COLOR, linewidth=0.55, alpha=0.9)
    axes[0].set_axisbelow(True)
    axes[0].spines[["top", "right"]].set_visible(False)
    axes[0].spines[["left", "bottom"]].set_color("#69717A")
    axes[0].tick_params(labelsize=7.4, colors=TEXT_COLOR)
    axes[0].legend(loc="lower left", frameon=False, fontsize=7.0, handlelength=2.2)

    metrics = [split_benefit_removed, liquidity_remaining]
    for threshold in sorted(THRESHOLD_COLORS):
        threshold_rows = sorted(
            (row for row in rows if int(row["cadence_threshold"]) == threshold),
            key=lambda row: int(row["cadence_wave_scaled"]),
        )
        waves = [float(row["cadence_wave_scaled"]) / 10_000.0 for row in threshold_rows]
        for axis, metric in zip(axes[1:], metrics, strict=True):
            axis.plot(
                waves,
                [metric(row) for row in threshold_rows],
                color=THRESHOLD_COLORS[threshold],
                marker=THRESHOLD_MARKERS[threshold],
                markersize=3.7,
                linewidth=1.25,
                label=f"{threshold} sells",
            )

    configure_axis(
        axes[1],
        "(b) Split mitigation",
        "Splitting benefit removed",
    )
    axes[1].set_ylim(-2, 72)
    axes[1].set_yticks([0, 20, 40, 60])

    configure_axis(
        axes[2],
        "(c) Vault liquidity left",
        "Vault liquidity remaining",
    )
    axes[2].set_ylim(8, 38)
    axes[2].set_yticks([10, 20, 30])
    axes[1].set_xlabel("Cadence strength\n(0 = off)", fontsize=7.9)
    axes[2].set_xlabel("Cadence strength\n(0 = off)", fontsize=7.9)

    reference = next(
        row
        for row in rows
        if int(row["cadence_threshold"]) == REFERENCE_THRESHOLD
        and float(row["cadence_wave_scaled"]) / 10_000.0 == REFERENCE_WAVE
    )
    for axis, metric in zip(axes[1:], metrics, strict=True):
        axis.scatter(
            REFERENCE_WAVE,
            metric(reference),
            marker="*",
            s=80,
            facecolor="#F0B429",
            edgecolor=TEXT_COLOR,
            linewidth=0.65,
            zorder=5,
        )

    figure.suptitle(
        "Cadence trade-off with the reference base curve",
        x=0.5,
        y=0.985,
        fontsize=10.0,
        fontweight="bold",
    )
    figure.text(
        0.5,
        0.925,
        "Fixed parameters: 7% peg haircut · exponent 2.5 · wall sensitivity 2.0",
        ha="center",
        fontsize=7.5,
    )
    figure.text(
        0.5,
        0.875,
        "Known trade-off: every successful sell advances cadence, so cheap sells can worsen a later quote.",
        ha="center",
        fontsize=7.4,
        fontweight="bold",
        color="#9A3412",
    )
    legend = [
        Line2D(
            [],
            [],
            color=THRESHOLD_COLORS[threshold],
            marker=THRESHOLD_MARKERS[threshold],
            linewidth=1.25,
            markersize=4,
            label=f"Full after {threshold} sells",
        )
        for threshold in sorted(THRESHOLD_COLORS)
    ]
    legend.append(
        Line2D(
            [],
            [],
            marker="*",
            linestyle="none",
            markerfacecolor="#F0B429",
            markeredgecolor=TEXT_COLOR,
            markersize=8,
            label="Reference setting",
        )
    )
    figure.legend(
        handles=legend,
        title="Panels (b) and (c): cadence penalty reaches its configured maximum:",
        loc="lower center",
        bbox_to_anchor=(0.5, 0.015),
        ncol=5,
        frameon=False,
        fontsize=7.2,
        title_fontsize=7.3,
        handletextpad=0.45,
        columnspacing=1.15,
    )
    figure.subplots_adjust(left=0.085, right=0.99, top=0.80, bottom=0.27, wspace=0.38)

    args.output_pdf.parent.mkdir(parents=True, exist_ok=True)
    figure.savefig(args.output_pdf, bbox_inches="tight", facecolor="white")
    figure.savefig(args.output_svg, bbox_inches="tight", facecolor="white")
    plt.close(figure)


if __name__ == "__main__":
    main()
