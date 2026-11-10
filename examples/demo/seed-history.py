#!/usr/bin/env python3
"""Writes a believable week of run history for the demo projects, so the
History tab has something to show on a fresh demo. Times are relative to now.

    examples/demo/seed-history.py "$SWITCHYARD_STATE_DIR"
"""

import json
import pathlib
import random
import sys
import time

state = pathlib.Path(sys.argv[1])
out = state / "history" / "runs.jsonl"
out.parent.mkdir(parents=True, exist_ok=True)
rng = random.Random(14)
now = time.time()
# Local midnight today, so "today" and "yesterday" land where they should.
lt = time.localtime(now)
midnight = now - (lt.tm_hour * 3600 + lt.tm_min * 60 + lt.tm_sec)
DAY = 86400.0
GB = 1024 ** 3
records = []
run_id = 7_300_000
shas = iter(f"{rng.getrandbits(28):07x}" for _ in range(1000))


def add(task, label, ended, steps, outcome="ok", code=0, error=None, message=None, queued=0.0,
        run_secs=0.0, peak=None, cap=None, params=None, sha=None):
    global run_id
    run_id += 1
    build = sum(s[1] for s in steps)
    started = ended - build - run_secs
    records.append({
        "run": run_id,
        "task": task,
        "label": label,
        "requested": started - queued,
        "started": started,
        "ended": ended,
        "outcome": outcome,
        "code": code,
        "message": message,
        "error": error,
        "steps": [{"name": n, "secs": s, "attempts": a} for n, s, a in steps],
        "queued_secs": queued,
        "build_secs": build,
        "run_secs": run_secs,
        "peak_mem": peak,
        "mem_cap": cap,
        "params": params or {},
        "variant": None,
        "git": {"sha": sha or next(shas), "branch": "main", "dirty": 0},
        "log": None,
    })


def jitter(x, f=0.08):
    return x * (1 + rng.uniform(-f, f))


# Today's story sits in the hours before now, whatever time it is.
def today(minutes_ago):
    return max(midnight + 60, now - minutes_ago * 60)


# storefront: steady all week, then today's builds are 38% slower in "next build".
for day in range(6, -1, -1):
    for k in range(rng.randint(3, 6) if day else 9):
        t = midnight - day * DAY + 9 * 3600 + k * 2400 + rng.uniform(0, 600) if day else today(230 - k * 25)
        if t > now - 300:
            continue
        nb = jitter(64) * (1.38 if day == 0 else 1)
        add("storefront", "storefront", t, [("install", jitter(38), 1), ("next build", nb, 1)],
            run_secs=jitter(900, .5), peak=int(jitter(2.1) * GB), cap=12 * GB, queued=0 if k % 3 else jitter(40))

# ledger-api: green most of the week; this morning a type error breaks it for
# 18 minutes, three red runs, then a fix.
err = {"index": 6, "text": "src/billing/invoice.ts(212,17): error TS2339: Property 'minorUnits' does not exist on type 'Money'.",
       "file": "src/billing/invoice.ts", "line": 212, "col": 17, "matcher": "typescript"}
for day in range(6, 0, -1):
    for k in range(rng.randint(2, 4)):
        t = midnight - day * DAY + 10 * 3600 + k * 3000
        add("ledger-api", "ledger-api", t, [("tsc", jitter(21), 1), ("docker build", jitter(48), 1)], peak=int(jitter(1.4) * GB), cap=12 * GB)
t0 = today(120)
add("ledger-api", "ledger-api", t0 - 3600, [("tsc", jitter(21), 1), ("docker build", jitter(48), 1)], peak=int(1.5 * GB), cap=12 * GB)
for i in range(3):
    add("ledger-api", "ledger-api", t0 + i * 360, [("tsc", jitter(19), 1)], outcome="failed", code=2, error=err,
        message="exit 2 in step 1 of 2", sha="a91e04c" if i == 0 else None, cap=12 * GB)
add("ledger-api", "ledger-api", t0 + 18 * 60, [("tsc", jitter(21), 1), ("docker build", jitter(48), 1)], sha="3f2a1c8",
    peak=int(1.5 * GB), cap=12 * GB)

# api-gateway: a flaky docker build that needs a second try now and then.
for day in range(5, -1, -1):
    for k in range(rng.randint(1, 3)):
        t = midnight - day * DAY + 11 * 3600 + k * 4000
        if t > now - 600:
            continue
        flaky = (day + k) % 3 == 0
        add("api-gateway", "api-gateway", t, [("cargo build", jitter(55), 1), ("docker build", jitter(30) * (2 if flaky else 1), 2 if flaky else 1)],
            run_secs=jitter(1800, .6), peak=int(jitter(3.2) * GB), cap=12 * GB)

# android: heavy, waits for memory once yesterday.
for day in (4, 2, 1):
    t = midnight - day * DAY + 15 * 3600
    add("android", "android", t, [("assembleDebug", jitter(190), 1)], queued=360 if day == 1 else jitter(20),
        peak=int(jitter(6.8) * GB), cap=16 * GB)

# load-test gate: regressed once yesterday.
add("load-test", "load-test", midnight - 3 * DAY + 22 * 3600, [("k6 run", 240, 1), ("gate", 2, 1)])
add("load-test", "load-test", midnight - DAY + 22 * 3600, [("k6 run", 251, 1), ("gate", 2, 1)], outcome="failed", code=1,
    error={"index": 40, "text": "error: p99 178ms is over the 149ms baseline (+19%)", "file": None, "line": None, "col": None, "matcher": "generic"},
    message="exit 1 in step 2 of 2")

# docs-site and migrations: quiet, green.
for day in range(6, -1, -2):
    t = midnight - day * DAY + 16 * 3600
    if t < now:
        add("docs-site", "docs-site", t, [("hugo build", jitter(9), 1)], run_secs=jitter(600, .5))
        add("migrations", "migrations", t + 300, [("migrate", jitter(4), 1)])

# preview: branches people asked for.
for i, branch in enumerate(["release/2.14", "feat/checkout-v2", "fix/tax-rounding"]):
    t = today(40) if i == 0 else midnight - i * DAY + 13 * 3600
    if t < now:
        add("preview", "preview", t, [("next build", jitter(70), 1)], run_secs=jitter(1200, .4), params={"branch": branch})

# payments: an upgrade this morning, health check passed.
t = today(260)
add("payments-upgrade", "payments upgrade", t, [("upgrade", 150, 1), ("check", 3, 1)])

records.sort(key=lambda r: r["ended"])
records = [r for r in records if r["ended"] < now]
with out.open("w") as f:
    for r in records:
        f.write(json.dumps(r) + "\n")
print(f"wrote {len(records)} runs to {out}")
