"""Pong's whole network frame, before and after the rollback library, for the rollback round's
measurements (`bench/determinism/results.md`, "The rollback library").

    python bench/rollback/network.py <kestrel.exe> <old Pong folder> <new Pong folder>

The old folder holds the pre-rewrite Pong (`git show 1b5e284:kestrel/demos/pong/<file>` for
`pong`, `game`, `peer`, `net` and `replay`), the new one today's `pong.kes` and `game.kes`; keep
both outside the synced checkout, since each run writes `pong.replay` beside its program. Both run
under the same `kestrel` with `kestrel run --release`, headless and quiet, poisoning off, for 1,
3,000, 60,000 and 600,000 frames, five rounds, old and new in turn within each round. It prints
each whole run's median and range in ms, the cost of one frame as (T(N) - T(1)) / (N - 1) from
the medians, and whether old and new printed the same lines.
"""
import os
import statistics
import subprocess
import sys
import time

FRAMES = [1, 3000, 60000, 600000]
ROUNDS = 5


def run(exe, folder, frames):
    env = {k: v for k, v in os.environ.items() if not k.startswith("KESTREL_")}
    env.update(KESTREL_HEADLESS=str(frames), KESTREL_HEADLESS_QUIET="1", KESTREL_POISON="0")
    t0 = time.perf_counter()
    out = subprocess.run([exe, "run", "--release", os.path.join(folder, "pong.kes")], env=env, cwd=folder,
                         capture_output=True, text=True)
    seconds = time.perf_counter() - t0
    if out.returncode != 0:
        sys.exit(f"{folder} {frames}: exit {out.returncode}\n{out.stderr}")
    return seconds * 1000, out.stdout


def main():
    exe, old, new = sys.argv[1:4]
    times = {(v, n): [] for v in ("old", "new") for n in FRAMES}
    outputs = {}
    for r in range(ROUNDS):
        for n in FRAMES:
            for v, folder in (("old", old), ("new", new)):
                ms, stdout = run(exe, folder, n)
                times[(v, n)].append(ms)
                outputs.setdefault((v, n), stdout)
        print(f"round {r + 1} done", file=sys.stderr)
    med = {k: statistics.median(t) for k, t in times.items()}
    print("frames | old ms | new ms | old us/frame | new us/frame | new / old")
    for n in FRAMES:
        o, w = times[("old", n)], times[("new", n)]
        line = f"{n} | {med[('old', n)]:.1f} ({min(o):.1f}-{max(o):.1f}) | {med[('new', n)]:.1f} ({min(w):.1f}-{max(w):.1f})"
        if n > 1:
            po = (med[("old", n)] - med[("old", 1)]) * 1000 / (n - 1)
            pw = (med[("new", n)] - med[("new", 1)]) * 1000 / (n - 1)
            line += f" | {po:.3f} | {pw:.3f} | {pw / po:.3f}"
        print(line)
    for n in FRAMES:
        same = outputs[("old", n)] == outputs[("new", n)]
        print(f"{n} frames, same output: {same}")
        if not same:
            print("old:\n" + outputs[("old", n)] + "new:\n" + outputs[("new", n)])


main()
