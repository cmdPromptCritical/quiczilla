"""Render the public v0.1.11 batched transfer benchmark as mobile-readable SVG.

Run from the repository root:
    python benchmarks/plot_transfer_performance.py

The figure deliberately uses one horizontal-bar panel per scenario. A shared
axis would make the many-small-files result unreadable beside a single file.
"""

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


OUTPUT = Path(__file__).with_name("transfer-performance-v0.1.11.svg")

# Median wall-clock measurements from three SHA-256-verified 1 GiB uploads over
# the public STUN/SSH route. Throughput is MiB/s; time is seconds.
SCENARIOS = (
    ("16,384 × 64 KiB · tar pipe", (27.78, 2.42, 15.22), (36.86, 423.25, 67.26)),
    ("102 × 10 MiB + 4 MiB · tar pipe", (27.30, 18.72, 18.09), (37.51, 54.71, 56.61)),
    ("1 × 1 GiB · file mode", (28.06, 19.85, 19.42), (36.49, 51.59, 52.74)),
)

TOOLS = ("Quiczilla", "SCP", "rsync")
COLORS = ("#2563eb", "#f59e0b", "#64748b")  # blue, amber, slate


def main() -> None:
    matplotlib.rcParams.update(
        {
            "svg.hashsalt": "quiczilla-v0.1.11",
            "font.family": "DejaVu Sans",
            "font.size": 12,
        }
    )
    figure, axes = plt.subplots(3, 1, figsize=(9.2, 12.7))
    figure.subplots_adjust(top=0.89, bottom=0.12, hspace=0.55)
    figure.patch.set_facecolor("white")
    figure.suptitle(
        "Public DDNS transfer benchmark — v0.1.11",
        fontsize=20,
        fontweight="bold",
    )
    figure.text(
        0.5,
        0.945,
        "Windows 11 → Ubuntu · 1 GiB per scenario · median of 3 verified uploads · higher is better",
        ha="center",
        va="top",
        fontsize=11,
        color="#475569",
    )

    for axis, (title, throughput, seconds) in zip(axes, SCENARIOS, strict=True):
        maximum = max(throughput)
        bars = axis.barh(TOOLS, throughput, color=COLORS, height=0.6, edgecolor="none")
        axis.invert_yaxis()
        axis.set_title(title, loc="left", fontsize=15, fontweight="bold", pad=10)
        axis.set_xlabel("Median end-to-end throughput (MiB/s)", labelpad=8)
        axis.set_xlim(0, maximum * 1.32)
        axis.xaxis.grid(True, color="#cbd5e1", linewidth=0.8, alpha=0.8)
        axis.set_axisbelow(True)
        for spine in ("top", "right", "left"):
            axis.spines[spine].set_visible(False)
        axis.spines["bottom"].set_color("#94a3b8")
        axis.tick_params(axis="y", length=0, labelsize=13)
        axis.tick_params(axis="x", colors="#475569")

        for bar, rate, duration in zip(bars, throughput, seconds, strict=True):
            axis.text(
                rate + maximum * 0.035,
                bar.get_y() + bar.get_height() / 2,
                f"{rate:.2f} MiB/s  ·  {duration:.2f} s",
                va="center",
                ha="left",
                fontsize=11,
                fontweight=("bold" if bar.get_y() == bars[0].get_y() else "normal"),
                color="#0f172a",
            )

    figure.text(
        0.5,
        0.022,
        "Quiczilla used STUN-assisted direct QUIC. SCP and rsync used SSH/TCP on the same public endpoint.\n"
        "Tar creation is excluded; transfer plus remote extraction is timed. Every destination tree was SHA-256 verified.",
        ha="center",
        va="bottom",
        fontsize=10,
        color="#475569",
    )
    figure.savefig(OUTPUT, format="svg", facecolor="white")
    print(f"Wrote {OUTPUT}")


if __name__ == "__main__":
    main()
