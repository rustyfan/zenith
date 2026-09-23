import argparse
import csv
import pathlib
import random
import statistics


def percentile(values, fraction):
    values = sorted(values)
    index = (len(values) - 1) * fraction
    low = int(index)
    high = min(low + 1, len(values) - 1)
    return values[low] + (values[high] - values[low]) * (index - low)


def summarize(path):
    with path.open(newline="", encoding="utf-8-sig") as source:
        rows = list(csv.DictReader(source))
    result = {"run": path.stem, "samples": len(rows)}
    for column in ["cpu_ms", "gpu_ms", "wall_ms", "allocations", "allocated_bytes"]:
        values = [float(row[column]) for row in rows]
        result[column + "_mean"] = statistics.mean(values)
        result[column + "_p50"] = statistics.median(values)
        result[column + "_p95"] = percentile(values, 0.95)
        result[column + "_p99"] = percentile(values, 0.99)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=pathlib.Path)
    parser.add_argument("--compare", nargs=2, metavar=("BASELINE", "CANDIDATE"))
    args = parser.parse_args()
    results = [summarize(path) for path in sorted(args.directory.glob("*.csv"))
               if ".scopes." not in path.name and path.name != "summary.csv"]
    with (args.directory / "summary.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(results[0]))
        writer.writeheader()
        writer.writerows(results)
    for row in results:
        print(f"{row['run']}: n={row['samples']} "
              f"CPU mean/p50/p95 {row['cpu_ms_mean'] * 1000:.1f}/{row['cpu_ms_p50'] * 1000:.1f}/{row['cpu_ms_p95'] * 1000:.1f} us; "
              f"GPU p50 {row['gpu_ms_p50'] * 1000:.1f} us; "
              f"wall p50 {row['wall_ms_p50'] * 1000:.1f} us; "
              f"alloc/frame {row['allocations_p50']:.0f}; bytes/frame {row['allocated_bytes_p50']:.0f}")
    if args.compare:
        before, after = args.compare
        indexed = {row["run"]: row for row in results}
        pairs = [(row, indexed[after + row["run"][len(before):]]) for row in results
                 if row["run"].startswith(before + "-") and after + row["run"][len(before):] in indexed]
        if len(pairs) < 2:
            raise SystemExit("Need at least two paired runs")
        changes = [(1 - b["cpu_ms_p50"] / a["cpu_ms_p50"]) * 100 for a, b in pairs]
        rng = random.Random(42)
        bootstrap = [statistics.mean(rng.choices(changes, k=len(changes))) for _ in range(20000)]
        print(f"Paired CPU median reduction ({len(pairs)} runs): {statistics.mean(changes):.2f}%; "
              f"95% run-bootstrap interval [{percentile(bootstrap, .025):.2f}, {percentile(bootstrap, .975):.2f}]%")
