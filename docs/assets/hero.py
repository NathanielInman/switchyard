#!/usr/bin/env python3
"""Writes docs/assets/hero.svg: the README's animated hero.

A 14 second loop on the same desktop as Roundhouse's hero, with both
siblings in the bar: the signal is clicked and the board springs out of it,
storefront finishes on the main line and android takes its place, ledger-api
fails, its log opens on the error, the error is copied, and a click outside
closes the board.

Pure CSS animation inside the SVG, so GitHub plays it in an <img>. With
reduced motion it is a still of the board mid-story.

    python3 docs/assets/hero.py
"""

import math
from pathlib import Path

W, H = 1200, 660
T = 14  # loop seconds

# Nord, matching the default waybar theme and Roundhouse's hero.
BG0, BG1 = "#14171c", "#0d0f12"
BAR, BAR_FG, BAR_DIM, BAR_LINE = "#282a2e", "#c5c8c6", "#7c838e", "#3b4048"
PANEL, PANEL_LINE = "#25282c", "#41464e"
TILE, TILE_LINE, WELL = "#2c2f34", "#363a40", "#16181b"
FG, DIM, IDLE = "#c5c8c6", "#7c838e", "#555b64"
FROST, GO, CAUTION, STOP, DARK = "#88c0d0", "#a3be8c", "#ebcb8b", "#bf616a", "#1d2024"
FONT = "JetBrains Mono, DejaVu Sans Mono, Menlo, Consolas, monospace"

BAR_Y, BAR_H = 616, 44
SIG_X, SIG_Y = 930, BAR_Y + BAR_H / 2
LAMP_X = 1004
BW, BH = 640, 396
BX, BY = min(max(SIG_X - BW / 2, 18), W - BW - 18), BAR_Y - 12 - BH

# The story, in seconds.
CLICK_SIG, OPEN = 1.0, 1.08
STORE_DONE = 4.2
FAIL = 5.6
CLICK_TILE = 7.0
CLICK_COPY = 9.4
CLICK_OUT, CLOSED = 11.0, 11.45
RESET = 13.4

out, kfs = [], []


def add(s):
    out.append(s)


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def pct(sec):
    return f"{max(0.0, min(100.0, sec / T * 100)):.2f}%"


def anim(prop, pts):
    """A keyframe set from (seconds, value) points; returns its name."""
    name = f"k{len(kfs)}"
    rows = " ".join(f"{pct(t)} {{ {prop}: {v}; }}" for t, v in pts)
    kfs.append(f"@keyframes {name} {{ {rows} }}")
    return name


def fade(spans, ramp=0.22):
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


def style(name, extra=""):
    return f'class="a" style="animation-name:{name};{extra}"'


def text(x, y, s, size=13, fill=FG, weight=400, anchor="start", attrs=""):
    add(f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}" font-weight="{weight}" text-anchor="{anchor}" {attrs}>{esc(s)}</text>')


def group(name, base_opacity=1, extra=""):
    add(f'<g {style(name, extra)} opacity="{base_opacity}">')


def progress(x, y, w, frac_from, frac_to, t0, t1, color, base):
    add(f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="4" rx="2" fill="{TILE_LINE}"/>')
    k = anim("transform", [(0, f"scaleX({frac_from})"), (t0, f"scaleX({frac_from})"), (t1, f"scaleX({frac_to})"), (T, f"scaleX({frac_to})")])
    add(f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="4" rx="2" fill="{color}" {style(k, "transform-box:fill-box;transform-origin:left center")} transform="translate({x:.1f},0) scale({base},1) translate({-x:.1f},0)"/>')


# ---------------------------------------------------------------- scene

css = f"""
.a {{ animation-duration: {T}s; animation-iteration-count: infinite; animation-timing-function: linear; animation-fill-mode: both; }}
.pulse {{ animation: pulse 1.4s ease-in-out infinite; }}
@keyframes pulse {{ 0%, 100% {{ opacity: 1; }} 50% {{ opacity: .35; }} }}
@media (prefers-reduced-motion: reduce) {{
  .a, .pulse {{ animation: none !important; }}
  #cursor, .ripple {{ display: none; }}
}}
"""

add(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="{FONT}" xml:space="preserve" role="img" aria-label="A waybar with the Switchyard signal and the Roundhouse lamp. A click on the signal opens the Switchyard board: storefront finishes on the main line and android takes its place, ledger-api fails, its log opens on the TypeScript error, and the error is copied.">')
STYLE_AT = len(out)
add("")  # the <style> goes here once every keyframe exists
add(f"""<defs>
<linearGradient id="wall" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{BG0}"/><stop offset="1" stop-color="{BG1}"/></linearGradient>
<radialGradient id="haze1" cx=".85" cy="0" r=".75"><stop offset="0" stop-color="{FROST}" stop-opacity=".13"/><stop offset="1" stop-color="{FROST}" stop-opacity="0"/></radialGradient>
<radialGradient id="haze2" cx=".05" cy="1" r=".6"><stop offset="0" stop-color="#b48ead" stop-opacity=".09"/><stop offset="1" stop-color="#b48ead" stop-opacity="0"/></radialGradient>
<filter id="shadow" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#000" flood-opacity=".65"/></filter>
<clipPath id="screen"><rect width="{W}" height="{H}" rx="18"/></clipPath>
</defs>""")
add('<g clip-path="url(#screen)">')
add(f'<rect width="{W}" height="{H}" fill="url(#wall)"/><rect width="{W}" height="{H}" fill="url(#haze1)"/><rect width="{W}" height="{H}" fill="url(#haze2)"/>')

# The editor the failure points into.
EX, EY, EW, EH = 36, 36, 520, 330
add(f'<rect x="{EX}" y="{EY}" width="{EW}" height="{EH}" rx="12" fill="#1a1d22" stroke="#2a2e35"/>')
add(f'<rect x="{EX}" y="{EY}" width="{EW}" height="30" rx="12" fill="#1f2328"/>')
for k, c in enumerate([STOP, CAUTION, GO]):
    add(f'<circle cx="{EX + 18 + k * 16}" cy="{EY + 15}" r="5" fill="{c}" fill-opacity=".55"/>')
text(EX + 22, EY + 58, "~/code/ledger-api  main ● 3", 13, DIM)
code = [
    ("208", "const subtotal = sumLines(invoice.lines);"),
    ("209", "if (subtotal < plan.minimumCharge) {"),
    ("210", "  invoice.adjustments.push("),
    ("211", "    minimumCharge(plan, subtotal),"),
    ("212", "  );"),
    ("213", "}"),
]
for i, (n, line) in enumerate(code):
    y = EY + 96 + i * 22
    text(EX + 22, y, n, 13, IDLE)
    text(EX + 64, y, line, 13, "#9aa5b4" if n != "209" else FG)
# The squiggle appears when the build fails.
k = fade([(FAIL + 0.2, RESET)])
add(f'<path d="M{EX + 192},{EY + 122} q4,-4 8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0 t8,0" fill="none" stroke="{STOP}" stroke-width="1.6" {style(k)} opacity="1"/>')

# A second window behind, as on Roundhouse's desktop.
add(f'<rect x="210" y="330" width="440" height="250" rx="12" fill="#1a1d22" stroke="#2a2e35" opacity=".85"/>')
for i, w in enumerate([300, 220, 340, 180, 260, 140]):
    add(f'<rect x="{232 + (i % 3) * 12}" y="{372 + i * 26}" width="{w}" height="9" rx="4" fill="#3b4252" opacity=".8"/>')

# ---------------------------------------------------------------- the bar
add(f'<rect x="0" y="{BAR_Y}" width="{W}" height="{BAR_H}" fill="{BAR}"/>')
add(f'<line x1="0" y1="{BAR_Y}" x2="{W}" y2="{BAR_Y}" stroke="#000" stroke-opacity=".4"/>')
mid = BAR_Y + BAR_H / 2 + 5
for i in range(6):
    x = 18 + i * 30
    if i == 3:
        add(f'<rect x="{x - 4}" y="{BAR_Y + 9}" width="26" height="26" rx="6" fill="{FROST}"/>')
    text(x + 9, mid, str(i + 1), 14, DARK if i == 3 else BAR_DIM, 700 if i == 3 else 400, "middle")
text(208, mid, "nvim  src/billing/invoice.ts", 14, BAR_FG)
for x, s in [(612, "♪ lo-fi radio"), (760, "vol 42%")]:
    text(x, mid, s, 14, BAR_DIM)
    add(f'<line x1="{x - 14}" y1="{BAR_Y + 10}" x2="{x - 14}" y2="{BAR_Y + BAR_H - 10}" stroke="{BAR_LINE}"/>')
add(f'<line x1="{SIG_X - 34}" y1="{BAR_Y + 10}" x2="{SIG_X - 34}" y2="{BAR_Y + BAR_H - 10}" stroke="{BAR_LINE}"/>')

# Switchyard's signal: lit amber while building, red once something fails.
k = fade([(CLICK_SIG + 0.05, CLICK_OUT + 0.1)], 0.15)
add(f'<rect x="{SIG_X - 22}" y="{BAR_Y + 7}" width="52" height="30" rx="7" fill="{FROST}" fill-opacity=".22" {style(k)} opacity="0"/>')
add(f'<rect x="{SIG_X - 8}" y="{SIG_Y - 13}" width="16" height="26" rx="6" fill="#1b1d20" stroke="#4c566a" stroke-width="1.4"/>')
for dy, c in [(-7, STOP), (0, CAUTION), (7, GO)]:
    add(f'<circle cx="{SIG_X}" cy="{SIG_Y + dy}" r="2.8" fill="{c}" opacity=".18"/>')
k = fade([(0, FAIL), (RESET, T)], 0.15)
add(f'<g {style(k)} opacity="0"><circle class="pulse" cx="{SIG_X}" cy="{SIG_Y}" r="2.8" fill="{CAUTION}"/></g>')
k = fade([(FAIL, RESET)], 0.15)
add(f'<g {style(k)} opacity="1"><circle cx="{SIG_X}" cy="{SIG_Y - 7}" r="2.8" fill="{STOP}"/>')
text(SIG_X + 13, mid, "1", 14, STOP, 700)
add("</g>")

# Roundhouse, the sibling, right beside it.
add(f'<line x1="{LAMP_X - 30}" y1="{BAR_Y + 10}" x2="{LAMP_X - 30}" y2="{BAR_Y + BAR_H - 10}" stroke="{BAR_LINE}"/>')
add(f'<circle cx="{LAMP_X}" cy="{SIG_Y}" r="7.5" fill="{GO}"/>')
add(f'<line x1="{LAMP_X + 30}" y1="{BAR_Y + 10}" x2="{LAMP_X + 30}" y2="{BAR_Y + BAR_H - 10}" stroke="{BAR_LINE}"/>')
text(W - 22, mid, "14:32", 14, BAR_FG, anchor="end")

# ---------------------------------------------------------------- the board
spring = anim("transform", [
    (0, "translateY(26px) scale(.06)"), (OPEN - 0.01, "translateY(26px) scale(.06)"),
    (OPEN + 0.10, "translateY(14px) scale(.5)"), (OPEN + 0.20, "translateY(4px) scale(.9)"),
    (OPEN + 0.30, "translateY(-1px) scale(1.035)"), (OPEN + 0.42, "translateY(0) scale(.988)"),
    (OPEN + 0.56, "scale(1.006)"), (OPEN + 0.72, "scale(1)"), (CLICK_OUT + 0.08, "scale(1)"),
    (CLOSED, "translateY(14px) scale(.4)"), (T, "translateY(14px) scale(.4)"),
])
show = anim("opacity", [(0, 0), (OPEN - 0.01, 0), (OPEN + 0.12, 0.85), (OPEN + 0.2, 1), (CLICK_OUT + 0.08, 1), (CLOSED, 0), (T, 0)])
add(f'<g class="a" style="animation-name:{spring},{show};transform-origin:{SIG_X}px {SIG_Y}px"><g transform="translate({BX:.1f},{BY:.1f})">')
add(f'<rect width="{BW}" height="{BH}" rx="16" fill="{PANEL}" stroke="{PANEL_LINE}" filter="url(#shadow)"/>')

# Tabs and filter
add(f'<rect x="12" y="13" width="108" height="28" rx="7" fill="#33373d"/>')
text(24, 32, "Projects", 13, FG)
text(96, 32, "8", 13, DIM)
for x, label, n in [(136, "Deploys", "4"), (236, "Desk", "1"), (312, "Shelf", "")]:
    text(x, 32, label, 13, DIM)
    if n:
        text(x + len(label) * 7.8 + 6, 32, n, 13, DIM)
# Roundhouse runs too, so the row ends with a link to its drawer, dotted
# with its lamp.
add(f'<circle cx="374" cy="27.5" r="3" fill="{GO}"/>')
text(383, 32, "Roundhouse", 13, DIM)
text(383 + 10 * 7.8 + 5, 32, "\u2197", 13, DIM)
add(f'<rect x="{BW - 128}" y="13" width="116" height="28" rx="8" fill="none" stroke="{PANEL_LINE}"/>')
text(BW - 114, 32, "/  to filter", 13, DIM)

# Main line strip
add(f'<rect x="12" y="52" width="{BW - 24}" height="32" rx="8" fill="{CAUTION}" fill-opacity=".07" stroke="{CAUTION}" stroke-opacity=".35"/>')
add(f'<circle class="pulse" cx="28" cy="68" r="5" fill="{CAUTION}"/>')
group(fade([(0, STORE_DONE)], 0.18), 0)
text(42, 73, "Main line: storefront · next build", 13, FG)
text(BW - 24, 73, "3.1 of 12 GB · android waiting", 12.5, DIM, anchor="end")
add("</g>")
group(fade([(STORE_DONE + 0.1, T)], 0.18), 1)
text(42, 73, "Main line: android · assembleDebug", 13, FG)
text(BW - 24, 73, "5.8 of 12 GB", 12.5, DIM, anchor="end")
add("</g>")

TW, TH = (BW - 24 - 3 * 10) / 4, 96
GX = [12 + i * (TW + 10) for i in range(4)]
GY = [96, 202]

grid_out = anim("opacity", [(0, 1), (CLICK_TILE + 0.05, 1), (CLICK_TILE + 0.3, 0), (T, 0)])
grid_move = anim("transform", [(0, "none"), (CLICK_TILE + 0.05, "none"), (CLICK_TILE + 0.3, "translateX(-36px)"), (T, "translateX(-36px)")])
add(f'<g class="a" style="animation-name:{grid_out},{grid_move}">')


def tile_frame(x, y, border=None, tint=None, letter=""):
    add(f'<rect x="{x:.1f}" y="{y}" width="{TW:.1f}" height="{TH}" rx="10" fill="{tint or TILE}" stroke="{border or TILE_LINE}"/>')
    if letter:
        add(f'<rect x="{x + TW - 24:.1f}" y="{y + 10}" width="16" height="16" rx="4" fill="{CAUTION}"/>')
        text(x + TW - 16, y + 22, letter, 11, DARK, 700, "middle")


def tile_text(x, y, name, desc, status, color, dot, hollow=False, pulse=False):
    if hollow:
        add(f'<circle cx="{x + 16:.1f}" cy="{y + 18}" r="4.5" fill="none" stroke="{dot}" stroke-width="2"/>')
    else:
        cls = ' class="pulse"' if pulse else ""
        add(f'<circle{cls} cx="{x + 16:.1f}" cy="{y + 18}" r="5" fill="{dot}"/>')
    text(x + 28, y + 23, name, 13, FG, 700)
    text(x + 12, y + 46, desc, 12, DIM)
    text(x + 12, y + 66, status, 12, color)


# a: ledger-api, idle, then failed
x, y = GX[0], GY[0]
group(fade([(0, FAIL), (RESET, T)], 0.2), 0)
tile_frame(x, y, letter="a")
tile_text(x, y, "ledger-api", "compose up", "idle · ok 1h ago", DIM, IDLE)
add("</g>")
group(fade([(FAIL, RESET)], 0.2), 1)
tile_frame(x, y, STOP, "#3a2a2d", "a")
tile_text(x, y, "ledger-api", "compose up", "failed · TS2339", STOP, STOP)
add("</g>")

# s: storefront, building on the main line, then ok
x, y = GX[1], GY[0]
group(fade([(0, STORE_DONE), (RESET, T)], 0.2), 0)
tile_frame(x, y, "#5c5440", letter="s")
tile_text(x, y, "storefront", "next build", "build 2/2 · 0:48", CAUTION, CAUTION, pulse=True)
progress(x + 12, y + 78, TW - 24, 0.3, 1.0, OPEN, STORE_DONE, CAUTION, 1)
add("</g>")
group(fade([(STORE_DONE, RESET)], 0.2), 1)
tile_frame(x, y, letter="s")
tile_text(x, y, "storefront", "next build", "ok · 2m 14s", GO, GO)
add("</g>")

# d: api-gateway, running with a memory sparkline
x, y = GX[2], GY[0]
tile_frame(x, y, "#3f5b63", letter="d")
tile_text(x, y, "api-gateway", "localhost:8080", "running · 1.2 G", FROST, FROST)
pts = [(x + 12 + i * (TW - 24) / 19, y + 88 - (6 + 5 * math.sin(i * 0.7) + i * 0.25)) for i in range(20)]
add(f'<polyline points="{" ".join(f"{a:.1f},{b:.1f}" for a, b in pts)}" fill="none" stroke="{FROST}" stroke-width="1.4"/>')

# f: e2e-suite, ok
x, y = GX[3], GY[0]
tile_frame(x, y, letter="f")
tile_text(x, y, "e2e-suite", "playwright", "ok · 1m 52s", GO, GO)

# g: android, queued heavy, then building
x, y = GX[0], GY[1]
group(fade([(0, STORE_DONE), (RESET, T)], 0.2), 0)
tile_frame(x, y, letter="g")
tile_text(x, y, "android", "APK → emulator", "queued · heavy", CAUTION, CAUTION, hollow=True)
add("</g>")
group(fade([(STORE_DONE, RESET)], 0.2), 1)
tile_frame(x, y, "#5c5440", letter="g")
tile_text(x, y, "android", "APK → emulator", "build 1/2 · 0:06", CAUTION, CAUTION, pulse=True)
progress(x + 12, y + 78, TW - 24, 0.0, 0.55, STORE_DONE + 0.1, CLICK_OUT, CAUTION, 0.4)
add("</g>")

for i, (name, desc, status, letter) in enumerate([
    ("docs-site", "hugo server", "idle · ok 2h ago", "h"),
    ("migrations", "staging db", "idle · ok 1d ago", "j"),
    ("load-test", "k6 · 500 users", "idle · 3d ago", "k"),
]):
    x, y = GX[i + 1], GY[1]
    tile_frame(x, y, letter=letter)
    tile_text(x, y, name, desc, status, DIM, IDLE)

text(12, 330, "Recent", 12.5, DIM)
cx = 72
for chip in ["rotate wallpaper", "deploy staging", "tokei"]:
    w = len(chip) * 7.4 + 22
    add(f'<rect x="{cx}" y="314" width="{w:.1f}" height="24" rx="12" fill="none" stroke="{PANEL_LINE}"/>')
    text(cx + w / 2, 330, chip, 12, FG, anchor="middle")
    cx += w + 8
add("</g>")

# The log view: opens on the first real error.
log_in = anim("opacity", [(0, 0), (CLICK_TILE + 0.15, 0), (CLICK_TILE + 0.42, 1), (T, 1)])
log_move = anim("transform", [(0, "translateX(36px)"), (CLICK_TILE + 0.15, "translateX(36px)"), (CLICK_TILE + 0.42, "none"), (T, "none")])
add(f'<g class="a" style="animation-name:{log_in},{log_move}" opacity="0">')
add(f'<circle cx="20" cy="110" r="4" fill="{STOP}"/>')
text(32, 115, "ledger-api", 14, FG, 700)
text(128, 115, "exit 1 in step 1 of 2", 13, STOP)
add(f'<rect x="12" y="126" width="{BW - 24}" height="176" rx="6" fill="{WELL}"/>')
log = [
    ("===== switchyard · ledger-api · 14:31:07 =====", FROST),
    ("===== step 1/2: tsc =====", FROST),
    ("  Resolving dependencies...", DIM),
    ("  Compiling unit 8 of 8", DIM),
    ("src/billing/invoice.ts(212,17): error TS2339:", FG),
    ("  Property 'minimumCharge' does not exist on 'Plan'.", FG),
    ("Found 1 error in src/billing/invoice.ts:212", DIM),
    ("===== exit 1 · step 1 of 2 · stopped =====", STOP),
]
add(f'<rect x="20" y="{132 + 4 * 19 + 3}" width="{BW - 40}" height="38" rx="3" fill="{STOP}" fill-opacity=".28"/>')
for i, (line, c) in enumerate(log):
    text(28, 148 + i * 19, line, 12.5, c)
bx = 12
for label, filled in [("y  copy error", True), ("e  open :212", False), ("r  run again", False), ("esc  back", False)]:
    w = len(label) * 7.6 + 24
    fill = STOP if filled else "none"
    add(f'<rect x="{bx:.1f}" y="312" width="{w:.1f}" height="26" rx="6" fill="{fill}" fill-opacity="{0.85 if filled else 1}" stroke="{STOP if filled else PANEL_LINE}"/>')
    text(bx + w / 2, 329, label, 12.5, DARK if filled else FG, 700 if filled else 400, "middle")
    bx += w + 8
add("</g>")

# Footer: key hints, then the copy confirmation.
add(f'<line x1="12" y1="{BH - 44}" x2="{BW - 12}" y2="{BH - 44}" stroke="{PANEL_LINE}"/>')
group(fade([(0, CLICK_COPY), (CLICK_COPY + 1.4, T)], 0.15), 1)
for x, key, what in [(16, "a-l", "run"), (96, "⇧", "variants"), (206, "/", "filter"), (296, "esc", "close")]:
    text(x, BH - 18, key, 12, FG, 700)
    text(x + len(key) * 7.6 + 8, BH - 18, what, 12, DIM)
add("</g>")
group(fade([(CLICK_COPY + 0.05, CLICK_COPY + 1.4)], 0.15), 0)
text(16, BH - 18, "✓ copied the error with 3 lines of context", 12.5, GO, 700)
add("</g>")
add("</g></g>")

# ---------------------------------------------------------------- cursor
tile_cx, tile_cy = BX + GX[0] + TW / 2, BY + GY[0] + TH / 2
copy_cx, copy_cy = BX + 60, BY + 325
clicks = [(CLICK_SIG, SIG_X, SIG_Y), (CLICK_TILE, tile_cx, tile_cy), (CLICK_COPY, copy_cx, copy_cy), (CLICK_OUT, 420, 250)]
for t, cx_, cy_ in clicks:
    k = anim("opacity", [(0, 0), (t - 0.01, 0), (t, 0.9), (t + 0.45, 0), (T, 0)])
    k2 = anim("transform", [(0, "scale(.3)"), (t, "scale(.3)"), (t + 0.45, "scale(1.6)"), (T, "scale(1.6)")])
    add(f'<circle class="a ripple" style="animation-name:{k},{k2};transform-box:fill-box;transform-origin:center" cx="{cx_:.1f}" cy="{cy_:.1f}" r="18" fill="none" stroke="{FROST}" stroke-width="2.5" opacity="0"/>')


def tr(x, y):
    return f"translate({x - 3:.1f}px, {y - 4:.1f}px)"


cur = anim("transform", [
    (0, tr(760, 360)), (CLICK_SIG - 0.15, tr(SIG_X, SIG_Y)), (3.0, tr(SIG_X, SIG_Y)),
    (3.8, tr(BX + 360, BY + 170)), (6.4, tr(BX + 360, BY + 170)),
    (CLICK_TILE - 0.15, tr(tile_cx, tile_cy)), (8.6, tr(tile_cx, tile_cy)),
    (CLICK_COPY - 0.15, tr(copy_cx, copy_cy)), (10.2, tr(copy_cx, copy_cy)),
    (CLICK_OUT - 0.15, tr(420, 250)), (12.0, tr(420, 250)), (T, tr(760, 360)),
])
add(f'<g id="cursor" class="a" style="animation-name:{cur};animation-timing-function:cubic-bezier(.45,0,.25,1);transform:{tr(tile_cx, tile_cy)}">')
add('<path d="M0,0 L0,22 L6,17 L10,26 L14,24 L10,15 L18,15 Z" fill="#eceff4" stroke="#1d2024" stroke-width="1.4" stroke-linejoin="round"/>')
add("</g>")
add("</g></svg>")

out[STYLE_AT] = "<style>" + css + "\n".join(kfs) + "</style>"
Path(__file__).with_name("hero.svg").write_text("\n".join(out) + "\n")
print(f"wrote hero.svg ({sum(len(s) + 1 for s in out) // 1024} KB, {len(kfs)} keyframe sets)")
