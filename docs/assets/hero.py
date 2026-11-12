#!/usr/bin/env python3
"""Writes docs/assets/hero.svg: the README's animated hero.

A 14 second loop on the same desktop as the siblings' heroes, under the
floating Platform bar. The signal says two things are building. A click
springs the board out of it: storefront finishes on the main line and
android takes its place, ledger-api fails, a click on its tile opens the log
on the TypeScript error (the board grows to fit it), and the error is copied.
A click outside closes the board with the signal still red, and a
right-click on the signal acknowledges the failure.

The board is drawn by the board_* functions below in its own coordinates
(the top-left of the panel) at its real size, with no bar or desktop state,
so other heroes can port it. Every task, name and number is made up.

Pure CSS animation inside the SVG, so GitHub plays it in an <img>. With
reduced motion it is a still of the board just after ledger-api fails.

    python3 docs/assets/hero.py
"""

import math
from pathlib import Path

W, H = 1200, 660
T = 14  # loop seconds
STILL = 6.2  # the moment the reduced-motion still shows

# Nord, matching the Platform waybar and the siblings' heroes.
BG0, BG1 = "#14171c", "#0d0f12"
PILL, PILL_LINE = "#24262a", "#3a404d"  # rgba(40,42,46,.84) and rgba(76,86,106,.55) over the wallpaper
HOVER, WS_ACTIVE = "rgba(129,161,193,.18)", "rgba(129,161,193,.26)"
BAR_FG, BRIGHT, BAR_DIM = "#c5c8c6", "#d8dee9", "#616e88"
FROST, GO, CAUTION, STOP, PURPLE, DARK = "#88c0d0", "#a3be8c", "#ebcb8b", "#bf616a", "#b48ead", "#1b1d20"
FONT = "JetBrains Mono, DejaVu Sans Mono, Menlo, Consolas, monospace"

# The board's own theme (crates/yard-board/src/style.css).
PANEL, PANEL_LINE = "#1e2024", "#41464e"
TILE, TILE_HOVER, TILE_FAILED = "#2a2d32", "#33373d", "#393238"  # failed: tile mixed 10% with stop
WELL, WELL_LINE = "#0f1012", "#262a2f"
FG, DIM, IDLE = "#c5c8c6", "#7c838e", "#555b64"
LOG_FG, LOG_BAD, ERR_BG, ERR_FG = "#9aa1aa", "#e0848c", "#4a2a2f", "#ffd5d9"

# The story, in seconds.
CLICK_SIG, OPEN = 1.0, 1.08
STORE_DONE = 4.2
FAIL = 5.6
CLICK_TILE = 7.0
CLICK_COPY = 9.2
TOAST_END = 10.6
CLICK_OUT, CLOSED = 11.0, 11.45
ACK = 12.2
RESET = 13.4


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class Pen:
    """Collects SVG. add() and text() match the dot-files bar hero's Svg."""

    def __init__(self):
        self.out = []

    def add(self, s):
        self.out.append(s)

    def text(self, x, y, s, size=13, fill=FG, weight=400, anchor="start", extra=""):
        self.add(f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}" font-weight="{weight}" text-anchor="{anchor}"{extra}>{esc(s)}</text>')


def cw(size):
    """The advance of one monospace character at `size` px."""
    return size * 0.602


def fit(s, size, room):
    """`s` cut with an ellipsis to fit `room` pixels, as GTK ellipsizes."""
    if len(s) * cw(size) <= room:
        return s
    n = max(0, int(room / cw(size)) - 1)
    return s[:n].rstrip() + "…"


# ================================================================ the board
#
# Everything from here to "the scene" draws Switchyard's board in its own
# coordinates: (0, 0) is the panel's top-left, sizes are the GTK board's at
# its default 13px monospace. The board is as wide as its tab row needs,
# since the panel never gets narrower than its content. Its text needs
# `white-space: pre` (or xml:space="preserve") so double spaces survive.

TABS = [("Projects", 8), ("Deploys", 4), ("Desk", 1), ("History", 0), ("Inspection", 2), ("Shelf", 0)]
SEARCH_W = 168


def tab_width(name, n):
    """A tab button: 10px padding, the name, and its count 5px after."""
    return 20 + len(name) * cw(13) + (5 + len(str(n)) * cw(13) if n else 0)


LINK_W = 20 + cw(9) + 5 + 10 * cw(13) + 5 + cw(13)  # "● Roundhouse ↗"
HEADER_W = sum(tab_width(*t) for t in TABS) + 2 * len(TABS) + LINK_W + 8 + SEARCH_W
BW = round(HEADER_W + 26)  # 12px padding and a 1px border each side

ML_Y = 52  # the main line strip, below the 30px header and 10px spacing
GRID_Y = 94  # the stack: tiles, or the log
GAP, TH = 10, 104
TW = (BW - 26 - 3 * GAP) / 4
GX = [13 + i * (TW + GAP) for i in range(4)]
GY = [GRID_Y, GRID_Y + TH + GAP]
RECENT_Y = GY[1] + TH + 10
BH_GRID = RECENT_Y + 22 + 10 + 31  # recent, spacing, footer
LOG_WELL_Y, LOG_WELL_H = GRID_Y + 28, 300
LOG_ACTIONS_Y = LOG_WELL_Y + LOG_WELL_H + 8
BH_LOG = LOG_ACTIONS_Y + 24 + 10 + 31  # actions, spacing, footer; no Recent
GROW = BH_LOG - BH_GRID  # the log page is taller, and the panel grows upward

LOG_ACTIONS = ["y  copy error", "e  open :212", "r  run again", "f  follow", "a  acknowledge", "esc  back"]


def action_x():
    xs, x = [], 13
    for label in LOG_ACTIONS:
        w = 22 + len(label) * cw(11.5)
        xs.append((x, w))
        x += w + 6
    return xs


# Where a story can click, in board coordinates. Tiles in grid mode;
# "copy" and "back" in log mode (heights BH_GRID and BH_LOG).
BOARD_SPOTS = {
    "roundhouse": (13 + sum(tab_width(*t) + 2 for t in TABS) + LINK_W / 2, 27),
    "tab:Inspection": (13 + sum(tab_width(*t) + 2 for t in TABS[:4]) + tab_width(*TABS[4]) / 2, 27),
    "filter": (BW - 13 - SEARCH_W / 2, 27),
    "copy": (action_x()[0][0] + action_x()[0][1] / 2, LOG_ACTIONS_Y + 12),
    "back": (action_x()[-1][0] + action_x()[-1][1] / 2, LOG_ACTIONS_Y + 12),
}
for _i, _name in enumerate(["storefront", "ledger-api", "api-gateway", "e2e-suite", "android", "docs-site", "migrations", "load-test"]):
    BOARD_SPOTS[f"tile:{_name}"] = (GX[_i % 4] + TW / 2, GY[_i // 4] + TH / 2)

PHASE_COLOR = {"idle": IDLE, "queued": CAUTION, "waiting": CAUTION, "building": CAUTION, "running": FROST, "ok": GO, "failed": STOP}
PHASE_BORDER = {"queued": (CAUTION, .22), "waiting": (CAUTION, .22), "building": (CAUTION, .40), "running": (FROST, .40), "ok": (GO, .45), "failed": (STOP, .55)}


def board_panel(pen, h):
    """The panel itself."""
    pen.add(f'<rect x=".5" y=".5" width="{BW - 1}" height="{h - 1}" rx="16" fill="{PANEL}" stroke="{PANEL_LINE}"/>')


def board_header(pen, active="Projects", roundhouse=GO):
    """The tab row: configured tabs, History, Inspection, Shelf, then the
    link to Roundhouse's drawer dotted with its lamp; the filter on the
    right. `roundhouse` is the lamp's color (None while it isn't running)."""
    x = 13
    for name, n in TABS:
        w = tab_width(name, n)
        on = name == active
        if on:
            pen.add(f'<rect x="{x:.1f}" y="14" width="{w:.1f}" height="26" rx="8" fill="{TILE_HOVER}"/>')
        pen.text(x + 10, 31.5, name, 13, FG if on else DIM)
        if n:
            pen.text(x + 10 + len(name) * cw(13) + 5, 31.5, str(n), 13, DIM)
        x += w + 2
    if roundhouse:
        pen.text(x + 10, 30.5, "●", 9, roundhouse)
        pen.text(x + 10 + cw(9) + 5, 31.5, "Roundhouse", 13, DIM)
        pen.text(x + 10 + cw(9) + 10 + 10 * cw(13), 31.5, "↗", 13, DIM)
    sx = BW - 13 - SEARCH_W
    pen.add(f'<rect x="{sx:.1f}" y="13.5" width="{SEARCH_W}" height="27" rx="8" fill="none" stroke="{PANEL_LINE}"/>')
    pen.text(sx + 10, 31.5, "/  to filter", 13, DIM)


def board_mainline(pen, label, used, cap, waiting=""):
    """The main line strip: lamp, "Main line: task · step", the memory
    gauge (90x6) and "used of cap", plus who waits."""
    w = BW - 24
    pen.add(f'<rect x="12.5" y="{ML_Y + .5}" width="{w - 1}" height="31" rx="9" fill="{CAUTION}" fill-opacity=".07" stroke="{CAUTION}" stroke-opacity=".25"/>')
    pen.text(23, ML_Y + 20.5, "●", 13, CAUTION)
    pen.text(23 + cw(13) + 9, ML_Y + 20.5, label, 13, FG)
    right = f"{used} of {cap}" + (f" · {waiting} waiting" if waiting else "")
    rx = 12 + w - 11
    pen.text(rx, ML_Y + 20.5, right, 13, DIM, anchor="end")
    gx = rx - len(right) * cw(13) - 9 - 90
    frac = float(used.split()[0]) / float(cap.split()[0])
    pen.add(f'<rect x="{gx:.1f}" y="{ML_Y + 13}" width="90" height="6" rx="3" fill="#fff" fill-opacity=".1"/>')
    pen.add(f'<rect x="{gx:.1f}" y="{ML_Y + 13}" width="{max(6, 90 * frac):.1f}" height="6" rx="3" fill="{CAUTION}"/>')


def board_tile(pen, x, y, t, progress_attrs=""):
    """One task tile. `t` has name, sub, status, phase and hint, and may
    have progress (0..1, the bar while building) and spark (memory history,
    while building or running). `progress_attrs` lets a story animate the
    bar's fill: given, the fill is drawn full width for it to scale."""
    phase = t["phase"]
    color = PHASE_COLOR.get(phase, IDLE)
    fill = TILE_FAILED if phase == "failed" else TILE
    line, alpha = PHASE_BORDER.get(phase, ("none", 0))
    pen.add(f'<rect x="{x + .5:.1f}" y="{y + .5}" width="{TW - 1:.1f}" height="{TH - 1}" rx="11" fill="{fill}" stroke="{line}" stroke-opacity="{alpha}"/>')
    lx, ly = x + 19, y + 19
    if phase == "queued":
        pen.add(f'<circle cx="{lx:.1f}" cy="{ly}" r="3.7" fill="none" stroke="{color}" stroke-width="1.8"/>')
    else:
        if phase != "idle":
            pen.add(f'<circle cx="{lx:.1f}" cy="{ly}" r="8.7" fill="{color}" fill-opacity=".16"/>')
        pulse = ' class="pulse"' if phase == "building" else ""
        pen.add(f'<circle{pulse} cx="{lx:.1f}" cy="{ly}" r="4.6" fill="{color}"/>')
    hint_w = 10 + cw(10.5)
    pen.text(x + 34, y + 24, fit(t["name"], 13.5, TW - 34 - 11 - hint_w - 7), 13.5, FG, 700)
    if t.get("hint"):
        hx = x + TW - 11 - hint_w
        pen.add(f'<rect x="{hx:.1f}" y="{y + 11}" width="{hint_w:.1f}" height="15" rx="4" fill="{CAUTION}"/>')
        pen.text(hx + hint_w / 2, y + 22.5, t["hint"], 10.5, DARK, 700, "middle")
    room = TW - 22
    pen.text(x + 11, y + 42, fit(t["sub"], 11, room), 11, DIM)
    pen.text(x + 11, y + 59, fit(t["status"], 11.5, room), 11.5, DIM if phase == "idle" else color)
    sy = y + 66
    if t.get("progress") is not None:
        pen.add(f'<rect x="{x + 11:.1f}" y="{y + 65}" width="{room:.1f}" height="3" rx="1.5" fill="#fff" fill-opacity=".08"/>')
        fw = room if progress_attrs else max(3, room * t["progress"])
        pen.add(f'<rect x="{x + 11:.1f}" y="{y + 65}" width="{fw:.1f}" height="3" rx="1.5" fill="{color}" {progress_attrs}/>')
        sy = y + 71
    if t.get("spark"):
        data, h = t["spark"], 22
        n = max(len(data), 30)
        step = room / (n - 1)
        top = max(data) * 1.15
        pts = [(x + 11 + room - step * (len(data) - 1 - i), sy + h - 1 - v / top * (h - 3)) for i, v in enumerate(data)]
        line_pts = " ".join(f"{a:.1f},{b:.1f}" for a, b in pts)
        pen.add(f'<line x1="{x + 11:.1f}" y1="{sy + h - .5}" x2="{x + 11 + room:.1f}" y2="{sy + h - .5}" stroke="#fff" stroke-opacity=".05"/>')
        pen.add(f'<path d="M{pts[0][0]:.1f},{sy + h} L{line_pts.replace(" ", " L")} L{pts[-1][0]:.1f},{sy + h} Z" fill="{color}" fill-opacity=".16"/>')
        pen.add(f'<polyline points="{line_pts}" fill="none" stroke="{color}" stroke-width="1.5" stroke-linejoin="round"/>')
        pen.add(f'<circle cx="{pts[-1][0]:.1f}" cy="{pts[-1][1]:.1f}" r="2.4" fill="{color}"/>')


def board_recent(pen, chips):
    """"Recent" and the last tasks run that aren't pinned on this tab."""
    pen.text(13, RECENT_Y + 15.5, "Recent", 13, DIM)
    x = 13 + 6 * cw(13) + 6
    for chip in chips:
        w = 22 + len(chip) * cw(11.5)
        pen.add(f'<rect x="{x:.1f}" y="{RECENT_Y + 1.5}" width="{w:.1f}" height="19" rx="9.5" fill="none" stroke="{PANEL_LINE}"/>')
        pen.text(x + w / 2, RECENT_Y + 15, chip, 11.5, FG, anchor="middle")
        x += w + 6


def board_footer(pen, h, text, color=DIM, rule=True):
    """The footer: key hints, the log's path, or a toast."""
    if rule:
        pen.add(f'<line x1="12" y1="{h - 30.5}" x2="{BW - 12}" y2="{h - 30.5}" stroke="{PANEL_LINE}"/>')
    pen.text(16, h - 12, text, 11, color)


GRID_HINTS = "a-z run   ⇧ variants   / filter   tab next   esc close"


def board_log(pen, name, message, lines, error_at, primary=True):
    """The log page in log mode: the head, the well opened on the first
    real error, and the actions. `lines` are (text, kind) with kind "mark",
    "bad" or ""; `error_at` is the error line's index."""
    pen.text(13, GRID_Y + 15, "●", 13, STOP)
    pen.text(13 + cw(13) + 8, GRID_Y + 15, name, 14, FG, 700)
    pen.text(13 + cw(13) + 16 + len(name) * cw(14), GRID_Y + 15, message, 13, STOP)
    pen.add(f'<rect x="12.5" y="{LOG_WELL_Y + .5}" width="{BW - 25}" height="{LOG_WELL_H - 1}" rx="8" fill="{WELL}" stroke="{WELL_LINE}"/>')
    for i, (s, kind) in enumerate(lines):
        y = LOG_WELL_Y + 26 + i * 16
        if i == error_at:
            pen.add(f'<rect x="31" y="{y - 12}" width="{BW - 62}" height="16" fill="{ERR_BG}"/>')
        fill = ERR_FG if i == error_at else {"mark": FROST, "bad": LOG_BAD}.get(kind, LOG_FG)
        pen.text(31, y, s, 11.5, fill)
    for (x, w), label in zip(action_x(), LOG_ACTIONS):
        filled = primary and label.startswith("y ")
        pen.add(f'<rect x="{x + .5:.1f}" y="{LOG_ACTIONS_Y + .5}" width="{w - 1:.1f}" height="23" rx="7" fill="{STOP if filled else "none"}" stroke="{STOP if filled else PANEL_LINE}"/>')
        pen.text(x + w / 2, LOG_ACTIONS_Y + 16, label, 11.5, DARK if filled else FG, 700 if filled else 400, "middle")


# The tasks, by state. Order and hints follow the pins.
def task(name, sub, status, phase, hint, **kw):
    return dict(name=name, sub=sub, status=status, phase=phase, hint=hint, **kw)


def spark(n, base, amp, seed):
    return [base + amp * (math.sin(i * 0.55 + seed) * .6 + math.sin(i * 1.7 + seed * 2) * .3) + i * .04 for i in range(n)]


TASKS = {
    "storefront": [
        task("storefront", "next build", "next build 2/2 · 0:48", "building", "a", progress=0.3, spark=spark(26, 3.1, .5, 1)),
        task("storefront", "next build", "ok · 2m 14s", "ok", "a"),
    ],
    "ledger-api": [
        task("ledger-api", "compose up", "tsc 1/2 · 0:31", "building", "s", progress=0.2),
        task("ledger-api", "compose up", "failed just now · TS2339", "failed", "s"),
        task("ledger-api", "compose up", "idle · failed just now", "idle", "s"),
    ],
    "api-gateway": [task("api-gateway", "localhost:8080", "running 42:10 · 1.2 GB", "running", "d", spark=spark(30, 1.2, .25, 3))],
    "e2e-suite": [task("e2e-suite", "playwright", "idle · ok 12m ago", "idle", "f")],
    "android": [
        task("android", "APK to emulator", "queued #1 · heavy", "queued", "g"),
        task("android", "APK to emulator", "assembleDebug · 0:06", "building", "g", progress=0.0),
    ],
    "docs-site": [task("docs-site", "hugo server", "idle · ok 2h ago", "idle", "h")],
    "migrations": [task("migrations", "staging db", "idle · ok yesterday", "idle", "j")],
    "load-test": [task("load-test", "k6 · 500 users", "idle · failed 3d ago", "idle", "k")],
}
ORDER = ["storefront", "ledger-api", "api-gateway", "e2e-suite", "android", "docs-site", "migrations", "load-test"]
RECENT = ["rotate wallpaper", "deploy staging", "release notes"]
MAINLINE = {
    "storefront": ("Main line: storefront · next build", "3.1 GB", "12 GB", "android"),
    "android": ("Main line: android · assembleDebug", "5.8 GB", "12 GB", ""),
}
LOG = [
    ("===== switchyard · ledger-api · 2026-11-10 14:31:07 =====", "mark"),
    ("===== step 1/2: tsc =====", "mark"),
    ("  Resolving dependencies...", ""),
    ("  Dependencies are up to date.", ""),
    ("  Compiling unit 4 of 8", ""),
    ("  Compiling unit 8 of 8", ""),
    ("src/billing/invoice.ts(212,17): error TS2339: Property 'minimumCharge' does not exist on type 'Plan'.", ""),
    ("", ""),
    ("Found 1 error in src/billing/invoice.ts:212", ""),
    ("===== exit 1 · step 1 of 2 · stopped =====", "bad"),
]
LOG_PATH = "/home/you/.local/state/switchyard/logs/ledger-api.log   t tail in a terminal"


def board(pen, state="grid", tiles=None, mainline="android"):
    """A whole board in one state, for a still: state "grid" or "log",
    `tiles` maps a task to its index in TASKS (default: just after
    ledger-api fails). Returns the board's height."""
    tiles = tiles or {"storefront": 1, "ledger-api": 1, "android": 1}
    h = BH_LOG if state == "log" else BH_GRID
    board_panel(pen, h)
    board_header(pen)
    board_mainline(pen, *MAINLINE[mainline])
    if state == "log":
        board_log(pen, "ledger-api", "exit 1 in step 1 of 2", LOG, 6)
        board_footer(pen, h, LOG_PATH)
    else:
        for i, name in enumerate(ORDER):
            board_tile(pen, GX[i % 4], GY[i // 4], TASKS[name][tiles.get(name, 0)])
        board_recent(pen, RECENT)
        board_footer(pen, h, GRID_HINTS)
    return h


# ---- end of the panel ----
# dot-files' bar hero (docs/bar/apps.py) runs everything above this line
# to draw the panel in its own tour, so keep the panel's code above it and
# the story below it.

# ================================================================ the scene

P = Pen()
add, text = P.add, P.text
kfs = []


def pct(sec):
    return f"{max(0.0, min(100.0, sec / T * 100)):.2f}%"


def keys(pts):
    """A keyframe set from (seconds, declarations) points; returns its name."""
    name = f"k{len(kfs)}"
    rows = " ".join(f"{pct(t)} {{ {v}; }}" for t, v in pts)
    kfs.append(f"@keyframes {name} {{ {rows} }}")
    return name


def anim(prop, pts):
    return keys([(t, f"{prop}: {v}") for t, v in pts])


def at(spans, t):
    return any(s <= t < e for s, e in spans)


def fade(spans, ramp=0.2):
    """Opacity keyframes visible during each (start, end) span."""
    pts = []
    for s, e in spans:
        if s <= 0:
            pts.append((0, 1))
        else:
            pts += [(0, 0)] if not pts else []
            pts += [(s, 0), (s + ramp, 1)]
        if e >= T:
            pts.append((T, 1))
        else:
            pts += [(e, 1), (e + ramp, 0)]
    if pts[-1][0] < T:
        pts.append((T, 0))
    return anim("opacity", pts)


def style(*names, extra=""):
    return f'class="a" style="animation-name:{",".join(names)};{extra}"'


def group(spans, ramp=0.2, still=None):
    """A group shown during `spans`; the still shows it if STILL is in them."""
    on = at(spans, STILL) if still is None else still
    add(f'<g {style(fade(spans, ramp))} opacity="{1 if on else 0}">')


css = f"""
text {{ white-space: pre; }}
.a {{ animation-duration: {T}s; animation-iteration-count: infinite; animation-timing-function: linear; animation-fill-mode: both; }}
.pulse {{ animation: pulse 1.4s ease-in-out infinite; }}
@keyframes pulse {{ 0%, 100% {{ opacity: 1; }} 50% {{ opacity: .45; }} }}
@media (prefers-reduced-motion: reduce) {{
  .a, .pulse {{ animation: none !important; }}
  #cursor, .ripple {{ display: none; }}
}}
"""

ALT = (
    "A floating waybar of frosted pills whose Switchyard signal is amber with a 2: two builds are going. A click "
    "springs the board out of it: Projects, Deploys, Desk, History, Inspection and Shelf tabs, a link to Roundhouse "
    "dotted with its lamp, the main line with its memory gauge, and eight tiles. storefront finishes on the main line "
    "and android takes its place, then ledger-api fails and the signal turns red. A click on its tile opens the log "
    "on the TypeScript error, and y copies it with context. The board closes with the signal still red until a "
    "right-click acknowledges the failure."
)

add(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="{FONT}" xml:space="preserve" role="img" aria-label="{esc(ALT)}">')
STYLE_AT = len(P.out)
add("")  # the <style> goes here once every keyframe exists
add(f"""<defs>
<linearGradient id="wall" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{BG0}"/><stop offset="1" stop-color="{BG1}"/></linearGradient>
<radialGradient id="haze1" cx=".85" cy="0" r=".75"><stop offset="0" stop-color="{FROST}" stop-opacity=".13"/><stop offset="1" stop-color="{FROST}" stop-opacity="0"/></radialGradient>
<radialGradient id="haze2" cx=".05" cy="1" r=".6"><stop offset="0" stop-color="{PURPLE}" stop-opacity=".09"/><stop offset="1" stop-color="{PURPLE}" stop-opacity="0"/></radialGradient>
<linearGradient id="dusk" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3b2b4a"/><stop offset=".7" stop-color="#b8706a"/><stop offset="1" stop-color="#d08770"/></linearGradient>
<filter id="shadow" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#000" flood-opacity=".65"/></filter>
<clipPath id="screen"><rect width="{W}" height="{H}" rx="18"/></clipPath>
<clipPath id="stack"><rect x="-40" y="{GRID_Y - GROW - 4}" width="{BW + 80}" height="{BH_GRID - 35 - GRID_Y + GROW}"/></clipPath>
<clipPath id="grow"><rect x="-40" y="{-GROW - 60}" width="{BW + 80}" height="{GROW + 60 + BH_GRID - 20}"/></clipPath>
</defs>""")
add('<g clip-path="url(#screen)">')
add(f'<rect width="{W}" height="{H}" fill="url(#wall)"/><rect width="{W}" height="{H}" fill="url(#haze1)"/><rect width="{W}" height="{H}" fill="url(#haze2)"/>')


def window(x, y, w, h):
    add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" fill="#1a1d22" stroke="#2a2e35"/>')
    add(f'<rect x="{x}" y="{y}" width="{w}" height="30" rx="12" fill="#1f2328"/>')
    for k, c in enumerate([STOP, CAUTION, GO]):
        add(f'<circle cx="{x + 18 + k * 16}" cy="{y + 15}" r="5" fill="{c}" fill-opacity=".55"/>')


# The editor the failure points into: nvim in kitty, in ledger-api.
EX, EY, EW, EH = 36, 36, 520, 330
window(EX, EY, EW, EH)
text(EX + 22, EY + 58, "~/code/ledger-api  main ● 3", 13, DIM)
code = [
    ("208", "export function chargeFor(invoice, plan: Plan) {"),
    ("209", "  const subtotal = sumLines(invoice.lines);"),
    ("210", "  if (invoice.trial) return 0;"),
    ("211", "  if (subtotal < 500) {"),
    ("212", "    return plan.minimumCharge;"),
    ("213", "  }"),
    ("214", "  return subtotal;"),
]
for i, (n, line) in enumerate(code):
    y = EY + 92 + i * 22
    text(EX + 22, y, n, 13, "#4c566a")
    text(EX + 64, y, line, 13, FG if n == "212" else "#9aa5b4")
# The squiggle under minimumCharge (212,17) from the failure on.
sx, sy = EX + 64 + 16 * cw(13), EY + 92 + 4 * 22 + 4
waves = " ".join("t6.5,0" for _ in range(15))
group([(FAIL + 0.2, RESET)])
add(f'<path d="M{sx:.1f},{sy} q3.25,-3.5 6.5,0 {waves}" fill="none" stroke="{STOP}" stroke-width="1.5"/>')
add("</g>")

# A browser behind it, as on the siblings' desktops.
window(216, 330, 440, 250)
for i, w in enumerate([300, 220, 340, 180, 260, 140]):
    add(f'<rect x="{238 + (i % 3) * 12}" y="{372 + i * 26}" width="{w}" height="9" rx="4" fill="#3b4252" opacity=".8"/>')

# ================================================================ the bar
#
# config.jsonc: height 34, 6px margins, 6px between pills. Left: workspaces
# and the window title. Right: media, lamps, audio, desk, clock, tray. The
# Nerd Font glyphs are drawn, since an <img> has no icon font.

BAR_Y, BAR_H = 620, 34
MID = BAR_Y + 21.5  # text baseline
CY = BAR_Y + BAR_H / 2
BS, BC = 13, cw(13)  # bar font size and advance
GLYPH = 14  # an icon's advance in the bar
MOD = 8 + GLYPH + 8  # a lamp module: 8px padding each side


def pill(x, w):
    add(f'<rect x="{x + .5:.1f}" y="{BAR_Y + .5}" width="{w - 1:.1f}" height="{BAR_H - 1}" rx="10" fill="{PILL}" stroke="{PILL_LINE}"/>')


def hover(x, w, spans):
    """A module's hover background (margin 3px inside its pill)."""
    group(spans, 0.1, False)
    add(f'<rect x="{x:.1f}" y="{BAR_Y + 4}" width="{w:.1f}" height="{BAR_H - 8}" rx="7" fill="{HOVER}"/>')
    add("</g>")


def g_dot(cx, c, r=6):
    add(f'<circle cx="{cx:.1f}" cy="{CY}" r="{r}" fill="{c}"/>')


def g_ring(cx, c):
    add(f'<circle cx="{cx:.1f}" cy="{CY}" r="5.6" fill="none" stroke="{c}" stroke-width="1.6"/>')


def g_puzzle(cx, c):
    """md-puzzle, Switchyard's signal in this bar (bar.icon)."""
    add(f'<path transform="translate({cx - 7.5:.1f},{CY - 7.6:.1f}) scale(.64)" fill="{c}" d="M20.5,11H19V7C19,5.89 18.1,5 17,5H13V3.5A2.5,2.5 0 0,0 10.5,1A2.5,2.5 0 0,0 8,3.5V5H4A2,2 0 0,0 2,7V10.8H3.5C5,10.8 6.2,12 6.2,13.5C6.2,15 5,16.2 3.5,16.2H2V20A2,2 0 0,0 4,22H7.8V20.5C7.8,19 9,17.8 10.5,17.8C12,17.8 13.2,19 13.2,20.5V22H17A2,2 0 0,0 19,20V16H20.5A2.5,2.5 0 0,0 23,13.5A2.5,2.5 0 0,0 20.5,11Z"/>')


def g_vpn_off(cx, c):
    """md-lan-disconnect: two screens and a cross."""
    x, y = cx - 7, CY - 7
    add(f'<g fill="none" stroke="{c}" stroke-width="1.4" stroke-linejoin="round">'
        f'<rect x="{x + .7:.1f}" y="{y + .7:.1f}" width="6.6" height="4.6" rx="1"/><path d="M{x:.1f},{y + 7.3:.1f} h8"/>'
        f'<rect x="{x + 6.7:.1f}" y="{y + 7.7:.1f}" width="6.6" height="4.6" rx="1"/><path d="M{x + 6:.1f},{y + 14.3:.1f} h8"/>'
        f'<path d="M{x + .8:.1f},{y + 9:.1f} l3.4,3.4 M{x + 4.2:.1f},{y + 9:.1f} l-3.4,3.4" stroke-linecap="round"/></g>')


def g_bell(cx, c):
    add(f'<path d="M{cx - 5},{CY + 3} L{cx - 5},{CY - 1} Q{cx - 5},{CY - 6} {cx},{CY - 6} Q{cx + 5},{CY - 6} {cx + 5},{CY - 1} L{cx + 5},{CY + 3} L{cx + 6.5},{CY + 4.5} L{cx - 6.5},{CY + 4.5} Z" fill="{c}"/>')
    add(f'<circle cx="{cx}" cy="{CY + 6.4}" r="1.7" fill="{c}"/>')


def g_speaker(cx, c):
    add(f'<path d="M{cx - 7},{CY - 2.5} L{cx - 4},{CY - 2.5} L{cx},{CY - 6} L{cx},{CY + 6} L{cx - 4},{CY + 2.5} L{cx - 7},{CY + 2.5} Z" fill="{c}"/>')
    add(f'<path d="M{cx + 3},{CY - 3} Q{cx + 5.5},{CY} {cx + 3},{CY + 3} M{cx + 5.5},{CY - 5.5} Q{cx + 10},{CY} {cx + 5.5},{CY + 5.5}" fill="none" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>')


def g_mouse(cx, c):
    add(f'<rect x="{cx - 5}" y="{CY - 7.5}" width="10" height="15" rx="5" fill="{c}"/><line x1="{cx}" y1="{CY - 7}" x2="{cx}" y2="{CY - 2}" stroke="{PILL}" stroke-width="1.4"/>')


def g_mic(cx, c):
    add(f'<g fill="none" stroke="{c}" stroke-width="1.6" stroke-linecap="round"><rect x="{cx - 3}" y="{CY - 8}" width="6" height="10" rx="3" fill="{c}"/><path d="M{cx - 6},{CY - 1} a6,6 0 0 0 12,0 M{cx},{CY + 5} v3"/></g>')


def g_chevron(cx, c):
    add(f'<path d="M{cx + 2.5},{CY - 5} l-5,5 l5,5" fill="none" stroke="{c}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>')


def g_window(kind, cx, c):
    """The workspace glyphs: a terminal, Chrome, Slack."""
    if kind == "term":
        add(f'<rect x="{cx - 6:.1f}" y="{CY - 5.5}" width="12" height="11" rx="2" fill="none" stroke="{c}" stroke-width="1.4"/>'
            f'<path d="M{cx - 3.5:.1f},{CY - 2} l2.5,2 l-2.5,2 M{cx + .5:.1f},{CY + 2.6} h3" fill="none" stroke="{c}" stroke-width="1.3" stroke-linecap="round"/>')
    elif kind == "chrome":
        add(f'<circle cx="{cx:.1f}" cy="{CY}" r="6" fill="none" stroke="{c}" stroke-width="1.5"/><circle cx="{cx:.1f}" cy="{CY}" r="2.3" fill="{c}"/>'
            f'<path d="M{cx:.1f},{CY - 6} v3.7 M{cx - 5.2:.1f},{CY + 3} l3.2,-1.8 M{cx + 5.2:.1f},{CY + 3} l-3.2,-1.8" stroke="{c}" stroke-width="1.3"/>')
    elif kind == "slack":
        add(f'<path d="M{cx - 2.2:.1f},{CY - 6} v12 M{cx + 2.2:.1f},{CY - 6} v12 M{cx - 6:.1f},{CY - 2.2} h12 M{cx - 6:.1f},{CY + 2.2} h12" stroke="{c}" stroke-width="2" stroke-linecap="round"/>')


def g_vim(cx, c):
    add(f'<path d="M{cx:.1f},{CY - 7} L{cx + 7:.1f},{CY} L{cx:.1f},{CY + 7} L{cx - 7:.1f},{CY} Z" fill="none" stroke="{c}" stroke-width="1.3"/>'
        f'<path d="M{cx - 3:.1f},{CY - 3} L{cx:.1f},{CY + 3} L{cx + 3:.1f},{CY - 3}" fill="none" stroke="{c}" stroke-width="1.5" stroke-linejoin="round"/>')


# ---- left: workspaces (a dim number, then a glyph per window) and the title
WS = [("1", ["term", "chrome"], True), ("2", ["slack"], False)]
x = 6 + 1 + 4
ws_w = sum(2 + 16 + BC * (1 + 2 * len(w)) for _, w, _ in WS) + 10
pill(6, ws_w)
for n, wins, active in WS:
    bw = 16 + BC * (1 + 2 * len(wins))
    x += 1
    if active:
        add(f'<rect x="{x:.1f}" y="{BAR_Y + 5}" width="{bw:.1f}" height="{BAR_H - 10}" rx="7" fill="{WS_ACTIVE}"/>')
    c = BRIGHT if active else BAR_FG
    text(x + 8, MID, n, BS, c, extra=' fill-opacity=".55"')
    for k, kind in enumerate(wins):
        g_window(kind, x + 8 + BC * (1 + 2 * k) + BC, c)
    x += bw + 1
TITLE = "~/code/ledger-api"
title_x = 6 + ws_w + 6
title_w = 24 + BC * (3 + len(TITLE)) + 2
pill(title_x, title_w)
g_vim(title_x + 13 + BC / 2, BRIGHT)
text(title_x + 13 + BC * 3, MID, TITLE, BS, BRIGHT)

# ---- right, packed from the edge
R = W - 6
TRAY_W = 2 + 10 + 16 + 8 + 16 + 10
tray_x = R - TRAY_W
pill(tray_x, TRAY_W)
for k, c in enumerate(["#81a1c1", "#5e81ac"]):
    add(f'<rect x="{tray_x + 11 + k * 24}" y="{BAR_Y + 9}" width="16" height="16" rx="4" fill="{c}" fill-opacity=".8"/>')

CLOCK = "Tue Nov 10  14:32"
clock_w = 2 + 24 + BC * len(CLOCK)
clock_x = tray_x - 6 - clock_w
pill(clock_x, clock_w)
text(clock_x + 13, MID, CLOCK, BS, PURPLE)

desk_w = 2 + 8 + MOD + (8 + BC + 8)
desk_x = clock_x - 6 - desk_w
pill(desk_x, desk_w)
g_mic(desk_x + 5 + 8 + GLYPH / 2, BAR_DIM)
g_chevron(desk_x + 5 + MOD + 8 + BC / 2, BAR_DIM)

AUDIO_MOD = 8 + GLYPH + BC + 3 * BC + 8
audio_w = 2 + 8 + 2 * AUDIO_MOD
audio_x = desk_x - 6 - audio_w
pill(audio_x, audio_w)
ax = audio_x + 5
g_speaker(ax + 8 + GLYPH / 2, FROST)
text(ax + 8 + GLYPH + BC, MID, "35%", BS, FROST)
ax += AUDIO_MOD
g_mouse(ax + 8 + GLYPH / 2, BAR_DIM)
text(ax + 8 + GLYPH + BC, MID, "80%", BS, BAR_DIM)

# The lamps pill: roundhouse, switchyard, broker, vpn, notifications. The
# signal shows a count while more than one task builds, which widens its
# module and pushes everything to its left.
COUNT_W = 2 * BC
lamps_r = audio_x - 6
lamps_w = 2 + 8 + 5 * MOD  # without the count
lamps_x = lamps_r - lamps_w
WIDE = [(0, FAIL), (RESET, T)]  # "2" beside the signal
narrow_at_still = not at(WIDE, STILL)
shift = anim("transform", [(0, f"translateX({-COUNT_W:.1f}px)"), (FAIL, f"translateX({-COUNT_W:.1f}px)"), (FAIL + 0.01, "none"),
                           (RESET, "none"), (RESET + 0.01, f"translateX({-COUNT_W:.1f}px)"), (T, f"translateX({-COUNT_W:.1f}px)")])
shift_still = "" if narrow_at_still else f' transform="translate({-COUNT_W:.1f},0)"'

lx = lamps_x + 5  # the first module's left edge (narrow)
sig_x = lx + MOD  # the signal module's left edge (narrow)
broker_x = sig_x + MOD
# The pill grows from its left: a cap that slides, the fixed body, and a
# seam filler while it is wide.
add(f'<g {style(shift)}{shift_still}><rect x="{lamps_x + .5:.1f}" y="{BAR_Y + .5}" width="40" height="{BAR_H - 1}" rx="10" fill="{PILL}" stroke="{PILL_LINE}"/></g>')
pill(lamps_x, lamps_w)
group(WIDE, 0.01)
add(f'<rect x="{lamps_x - COUNT_W + 8:.1f}" y="{BAR_Y + 1}" width="{COUNT_W + 4:.1f}" height="{BAR_H - 2}" fill="{PILL}"/>')
for yy in (BAR_Y, BAR_Y + BAR_H - 1):
    add(f'<rect x="{lamps_x - COUNT_W + 8:.1f}" y="{yy}" width="{COUNT_W + 4:.1f}" height="1" fill="{PILL_LINE}"/>')
add("</g>")

# Hover under the signal while the cursor is on it (wide on open, narrow on
# the right-click).
hover(sig_x - COUNT_W, MOD + COUNT_W, [(CLICK_SIG - 0.15, 2.4)])
hover(sig_x, MOD, [(ACK - 0.15, ACK + 0.6)])

add(f'<g {style(shift)}{shift_still}>')
g_dot(lx + MOD / 2, GO)  # Roundhouse, calm
add("</g>")
# Switchyard's signal. Amber "2" while two build, red alone once ledger-api
# fails, amber again once the failure is acknowledged.
SIG = [
    ([(0, FAIL), (RESET, T)], CAUTION, True),
    ([(FAIL, ACK)], STOP, False),
    ([(ACK, RESET)], CAUTION, False),
]
for spans, c, count in SIG:
    group(spans, 0.05)
    if count:
        g_puzzle(sig_x - COUNT_W + 8 + GLYPH / 2, c)
        text(sig_x - COUNT_W + 8 + GLYPH + BC, MID, "2", BS, c)
    else:
        g_puzzle(sig_x + 8 + GLYPH / 2, c)
    add("</g>")
k = anim("opacity", [(0, 0), (FAIL, 0), (FAIL + 0.05, .8), (FAIL + 0.7, 0), (T, 0)])
k2 = anim("transform", [(0, "scale(.4)"), (FAIL, "scale(.4)"), (FAIL + 0.7, "scale(1.6)"), (T, "scale(1.6)")])
add(f'<circle class="a ripple" style="animation-name:{k},{k2};transform-box:fill-box;transform-origin:center" opacity="0" cx="{sig_x + MOD / 2:.1f}" cy="{CY}" r="12" fill="none" stroke="{STOP}" stroke-width="2"/>')
g_ring(broker_x + MOD / 2, BAR_DIM)  # Signal Box, nothing granted
g_vpn_off(broker_x + MOD * 1.5, BAR_DIM)  # VPN, disconnected
g_bell(broker_x + MOD * 2.5, BAR_DIM)  # notifications, none waiting

# The media pill: Tannoy's cover, then the song.
SONG = "Marlow Fen · Paper Lanterns"
media_w = 1 + 10 + 18 + 8 + BC * len(SONG) + 12 + 1
media_x = lamps_x - 6 - media_w
add(f'<g {style(shift)}{shift_still}>')
pill(media_x, media_w)
cx0 = media_x + 11
add(f'<rect x="{cx0}" y="{BAR_Y + 8}" width="18" height="18" rx="3" fill="url(#dusk)"/>')
for dx, dy, r in [(5, 7, 2.2), (11, 5, 2.6), (14.5, 9, 1.8)]:
    add(f'<circle cx="{cx0 + dx}" cy="{BAR_Y + 8 + dy}" r="{r}" fill="{CAUTION}"/>')
text(cx0 + 18 + 8, MID, SONG, BS, CAUTION)
add("</g>")
assert media_x - COUNT_W > title_x + title_w + 6, "the bar's two ends overlap"

# ================================================================ the board, on screen

SIG_CX, SIG_CX_WIDE = sig_x + MOD / 2, sig_x - COUNT_W + MOD / 2
BOT = BAR_Y - 8  # the board's gap above the bar
BX = min(max(SIG_CX_WIDE - BW / 2, 10), W - 10 - BW)
BY = BOT - BH_GRID
LOG_IN, LOG_DONE = CLICK_TILE + 0.05, CLICK_TILE + 0.33

spring = anim("transform", [
    (0, "translateY(26px) scale(.06)"), (OPEN - 0.01, "translateY(26px) scale(.06)"),
    (OPEN + 0.10, "translateY(14px) scale(.5)"), (OPEN + 0.20, "translateY(4px) scale(.9)"),
    (OPEN + 0.30, "translateY(-1px) scale(1.035)"), (OPEN + 0.42, "translateY(0) scale(.988)"),
    (OPEN + 0.56, "scale(1.006)"), (OPEN + 0.72, "scale(1)"), (CLICK_OUT + 0.08, "scale(1)"),
    (CLOSED, "translateY(14px) scale(.4)"), (T, "translateY(14px) scale(.4)"),
])
show = anim("opacity", [(0, 0), (OPEN - 0.01, 0), (OPEN + 0.12, 0.85), (OPEN + 0.2, 1), (CLICK_OUT + 0.08, 1), (CLOSED, 0), (T, 0)])
add(f'<g class="a" style="animation-name:{spring},{show};transform-origin:{SIG_CX_WIDE:.1f}px {CY}px"><g transform="translate({BX:.1f},{BY:.1f})">')


def grow(still_up=False):
    """Moves a group up by GROW while the log is open."""
    k = anim("transform", [(0, "none"), (LOG_IN, "none"), (LOG_DONE, f"translateY({-GROW}px)"), (T, f"translateY({-GROW}px)")])
    base = f' transform="translate(0,{-GROW})"' if still_up else ""
    return f'{style(k, extra="animation-timing-function:cubic-bezier(.3,0,.2,1)")}{base}'


# The panel grows upward when the log opens: a tall body that slides up,
# clipped above a fixed bottom cap.
add('<g filter="url(#shadow)">')
k = anim("transform", [(0, f"translateY({GROW}px)"), (LOG_IN, f"translateY({GROW}px)"), (LOG_DONE, "none"), (T, "none")])
add(f'<g clip-path="url(#grow)"><g {style(k, extra="animation-timing-function:cubic-bezier(.3,0,.2,1)")} transform="translate(0,{GROW})">')
add(f'<rect x=".5" y="{-GROW + .5}" width="{BW - 1}" height="{BH_LOG - 1}" rx="16" fill="{PANEL}" stroke="{PANEL_LINE}"/>')
add("</g></g>")
add(f'<rect x="0" y="{BH_GRID - 36}" width="{BW}" height="36" rx="16" fill="{PANEL}"/>')
add(f'<path d="M.5,{BH_GRID - 36} V{BH_GRID - 16} A15.5,15.5 0 0 0 16,{BH_GRID - .5} H{BW - 16} A15.5,15.5 0 0 0 {BW - .5},{BH_GRID - 16} V{BH_GRID - 36}" fill="none" stroke="{PANEL_LINE}"/>')
add("</g>")

# Header and main line ride up with the panel's top.
add(f"<g {grow()}>")
board_header(P)
group([(0, STORE_DONE), (RESET, T)], 0.18)
board_mainline(P, *MAINLINE["storefront"])
add("</g>")
group([(STORE_DONE, RESET)], 0.18)
board_mainline(P, *MAINLINE["android"])
add("</g>")
add("</g>")

# The stack: the grid slides out the top as the log comes up from below,
# both inside the stack's bounds.
add('<g clip-path="url(#stack)">')
grid_out = anim("opacity", [(0, 1), (LOG_IN, 1), (LOG_IN + 0.2, 0), (T, 0)])
grid_move = anim("transform", [(0, "none"), (LOG_IN, "none"), (LOG_IN + 0.25, "translateY(-24px)"), (T, "translateY(-24px)")])
add(f'<g class="a" style="animation-name:{grid_out},{grid_move}">')


def progress_anim(frac_from, frac_to, t0, t1, still):
    k = anim("transform", [(0, f"scaleX({frac_from})"), (t0, f"scaleX({frac_from})"), (t1, f"scaleX({frac_to})"), (T, f"scaleX({frac_to})")])
    return f'class="a" style="animation-name:{k};transform-box:fill-box;transform-origin:left center;transform:scaleX({still:.3f})"'


def tile_states(i, states):
    """Draws a tile's states, each shown during its spans."""
    x, y = GX[i % 4], GY[i // 4]
    for t, spans, prog in states:
        group(spans, 0.2)
        if prog:
            a, b, t0, t1 = prog
            still = a + (b - a) * min(1, max(0, (STILL - t0) / (t1 - t0)))
            board_tile(P, x, y, dict(t, progress=still), progress_anim(a, b, t0, t1, still))
        else:
            board_tile(P, x, y, t)
        add("</g>")


TK = TASKS
tile_states(0, [(TK["storefront"][0], [(0, STORE_DONE), (RESET, T)], (0.3, 1.0, OPEN, STORE_DONE)),
                (TK["storefront"][1], [(STORE_DONE, RESET)], None)])
tile_states(1, [(TK["ledger-api"][0], [(0, FAIL), (RESET, T)], (0.2, 0.55, OPEN, FAIL)),
                (TK["ledger-api"][1], [(FAIL, RESET)], None)])
tile_states(2, [(TK["api-gateway"][0], [(0, T)], None)])
tile_states(3, [(TK["e2e-suite"][0], [(0, T)], None)])
tile_states(4, [(TK["android"][0], [(0, STORE_DONE), (RESET, T)], None),
                (TK["android"][1], [(STORE_DONE, RESET)], (0.03, 0.4, STORE_DONE, CLICK_OUT))])
for i in range(5, 8):
    tile_states(i, [(TK[ORDER[i]][0], [(0, T)], None)])
board_recent(P, RECENT)
add("</g>")

# The log page, at the grown panel's coordinates.
log_in = anim("opacity", [(0, 0), (LOG_IN + 0.1, 0), (LOG_DONE, 1), (T, 1)])
log_move = anim("transform", [(0, f"translateY({-GROW + 24}px)"), (LOG_IN + 0.1, f"translateY({-GROW + 24}px)"), (LOG_DONE, f"translateY({-GROW}px)"), (T, f"translateY({-GROW}px)")])
add(f'<g class="a" style="animation-name:{log_in},{log_move}" opacity="0" transform="translate(0,{-GROW})">')
board_log(P, "ledger-api", "exit 1 in step 1 of 2", LOG, 6)
# y flashes under the click.
k = anim("opacity", [(0, 0), (CLICK_COPY - 0.01, 0), (CLICK_COPY, .5), (CLICK_COPY + 0.35, 0), (T, 0)])
x0, w0 = action_x()[0]
add(f'<rect class="a" style="animation-name:{k}" opacity="0" x="{x0}" y="{LOG_ACTIONS_Y}" width="{w0:.1f}" height="24" rx="7" fill="#fff"/>')
add("</g></g>")

# The footer stays put at the bottom: key hints, then the log's path, the
# toast, and the path again.
add(f'<line x1="12" y1="{BH_GRID - 30.5}" x2="{BW - 12}" y2="{BH_GRID - 30.5}" stroke="{PANEL_LINE}"/>')
for spans, s, c in [
    ([(0, LOG_IN), (RESET, T)], GRID_HINTS, DIM),
    ([(LOG_IN, CLICK_COPY), (TOAST_END, RESET)], LOG_PATH, DIM),
    ([(CLICK_COPY, TOAST_END)], "Copied the error with context", FG),
]:
    group(spans, 0.02)  # GTK swaps the label, it does not fade
    board_footer(P, BH_GRID, s, c, rule=False)
    add("</g>")
add("</g></g>")

# ================================================================ cursor

tile_cx, tile_cy = BX + BOARD_SPOTS["tile:ledger-api"][0], BY + BOARD_SPOTS["tile:ledger-api"][1]
copy_cx, copy_cy = BX + action_x()[0][0] + action_x()[0][1] - 16, BY - GROW + BOARD_SPOTS["copy"][1]
OUT_X, OUT_Y = 120, 470
clicks = [(CLICK_SIG, SIG_CX_WIDE, CY), (CLICK_TILE, tile_cx, tile_cy), (CLICK_COPY, copy_cx, copy_cy), (CLICK_OUT, OUT_X, OUT_Y), (ACK, SIG_CX, CY)]
for t, cx_, cy_ in clicks:
    k = anim("opacity", [(0, 0), (t - 0.01, 0), (t, 0.9), (t + 0.45, 0), (T, 0)])
    k2 = anim("transform", [(0, "scale(.3)"), (t, "scale(.3)"), (t + 0.45, "scale(1.6)"), (T, "scale(1.6)")])
    add(f'<circle class="a ripple" style="animation-name:{k},{k2};transform-box:fill-box;transform-origin:center" cx="{cx_:.1f}" cy="{cy_:.1f}" r="18" fill="none" stroke="{FROST}" stroke-width="2.5" opacity="0"/>')


def tr(x, y):
    return f"translate({x - 3:.1f}px, {y - 4:.1f}px)"


rest = (BX + BW - 150, BY + RECENT_Y + 12)
cur = anim("transform", [
    (0, tr(760, 330)), (CLICK_SIG - 0.15, tr(SIG_CX_WIDE, CY)), (2.4, tr(SIG_CX_WIDE, CY)),
    (3.2, tr(*rest)), (6.4, tr(*rest)),
    (CLICK_TILE - 0.15, tr(tile_cx, tile_cy)), (8.4, tr(tile_cx, tile_cy)),
    (CLICK_COPY - 0.15, tr(copy_cx, copy_cy)), (10.0, tr(copy_cx, copy_cy)),
    (CLICK_OUT - 0.15, tr(OUT_X, OUT_Y)), (11.4, tr(OUT_X, OUT_Y)),
    (ACK - 0.15, tr(SIG_CX, CY)), (ACK + 0.5, tr(SIG_CX, CY)), (T, tr(760, 330)),
])
add(f'<g id="cursor" class="a" style="animation-name:{cur};animation-timing-function:cubic-bezier(.45,0,.25,1);transform:{tr(*rest)}">')
add('<path d="M0,0 L0,22 L6,17 L10,26 L14,24 L10,15 L18,15 Z" fill="#eceff4" stroke="#1d2024" stroke-width="1.4" stroke-linejoin="round"/>')
add("</g>")
add("</g></svg>")

P.out[STYLE_AT] = "<style>" + css + "\n".join(kfs) + "</style>"
Path(__file__).with_name("hero.svg").write_text("\n".join(P.out) + "\n")
print(f"wrote hero.svg ({sum(len(s) + 1 for s in P.out) // 1024} KB, {len(kfs)} keyframe sets, board {BW}x{BH_GRID}, {BW}x{BH_LOG} with the log)")
