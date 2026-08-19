---
name: DSHL
description: DeepSeek Harness Launcher — Swiss International launch console, light and dark
colors:
  ultramarine: "#2f6fe4"
  ultramarine-strong: "#2258c4"
  ultramarine-soft: "#2457c4"
  accent-ink: "#ffffff"
  ground: "#ffffff"
  ground-dark: "#0f0f11"
  surface: "#f6f6f4"
  surface-dark: "#1a1a1d"
  ink: "#161616"
  ink-dark: "#f0f0f1"
  muted: "#5f5f66"
  muted-dark: "#9c9ca4"
  hairline: "#e5e5e2"
  hairline-dark: "#2e2e33"
  signal-ok: "#0b7c4e"
  signal-ok-dark: "#3ccb8b"
  signal-warn: "#a8620a"
  signal-warn-dark: "#e2a03a"
  signal-err: "#c6352e"
  signal-err-dark: "#f06a6a"
typography:
  display:
    fontFamily: '"Inter", "Segoe UI", system-ui, -apple-system, "Microsoft YaHei", sans-serif'
    fontSize: "24px"
    fontWeight: 600
    lineHeight: 1
    letterSpacing: "0.02em"
  title:
    fontFamily: '"Inter", "Segoe UI", system-ui, -apple-system, "Microsoft YaHei", sans-serif'
    fontSize: "11px"
    fontWeight: 600
    letterSpacing: "0.12em"
  body:
    fontFamily: '"Inter", "Segoe UI", system-ui, -apple-system, "Microsoft YaHei", sans-serif'
    fontSize: "14px"
    fontWeight: 500
    lineHeight: 1.35
  label:
    fontFamily: '"Inter", "Segoe UI", system-ui, -apple-system, "Microsoft YaHei", sans-serif'
    fontSize: "12px"
    fontWeight: 600
    letterSpacing: "0.06em"
  data:
    fontFamily: '"JetBrains Mono", "Cascadia Code", Consolas, monospace'
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.65
rounded:
  none: "0px"
spacing:
  xs: "8px"
  sm: "12px"
  md: "16px"
  lg: "28px"
  xl: "40px"
  section: "44px"
components:
  button-primary:
    backgroundColor: "{colors.ultramarine}"
    textColor: "{colors.accent-ink}"
    rounded: "{rounded.none}"
    padding: "10px 18px"
  button-primary-hover:
    backgroundColor: "{colors.ultramarine-strong}"
    textColor: "{colors.accent-ink}"
  button:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "10px 18px"
  button-danger:
    backgroundColor: "transparent"
    textColor: "{colors.signal-err}"
    rounded: "{rounded.none}"
    padding: "10px 18px"
  log:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "16px 18px"
  status-stamp:
    textColor: "{colors.ultramarine-soft}"
    size: "11px"
---

# Design System: DSHL

## Overview

**Creative North Star: "The Ruled Control Panel"**

DSHL is the launch console of a developer tool rendered in the Swiss
International tradition: calm, objective, and ruled by the grid. It refuses
both the decorated product dashboard and its own previous heavy brutalism.
There is no decoration to admire — only measured type on a clean ground,
hairline rules that hold content in place, one ultramarine accent that says
*this is the live thing*, and a checklist that reads like a stamped sequence.

The page has no fixed mood: it follows the operating system. On a white ground
it is a printed program card; on a near-black ground it is a darkroom control
panel — the same grid, the same type, the same single accent. Status green,
amber, and red are functional signals, never decoration. Depth is conveyed by
whitespace and hairlines alone; nothing is shadowed, nothing is rounded.

**Key Characteristics:**
- One accent. Ultramarine (#2F6FE4) marks the primary action, the running state, and key values; green/amber/red report status.
- The grid does the work: hairline rules (1px), tracked uppercase section labels, tabular numerals, generous whitespace.
- Square corners, no shadows, no gradients — flat Swiss paint.
- The display voice is the self-hosted Inter grotesque (the style's native Helvetica is licensed; Inter is the closest self-hostable face).
- Light and dark are both authored, not defaulted — the page follows the OS via `prefers-color-scheme`.
- Calm motion, reserved for live states: the log's block cursor blinks, and the running step core plus the status LED breathe while the launch is in progress.

## Colors

Two authored grounds, one ultramarine accent, and a strict functional signal set. Every color has a light and a dark value; the page swaps the whole set on `prefers-color-scheme`.

### Primary
- **Ultramarine** (#2f6fe4; dark #3a76e6): the only brand accent. Fills the primary button, marks the running step number, and tints key config values.
- **Ultramarine Strong** (#2258c4): the primary button's hover/press.
- **Ultramarine Soft** (#2457c4 on white, #8aaaf7 on dark): the accent tuned for text legibility — config values, status stamp text.
- **Accent Ink** (#ffffff): text placed on ultramarine fills.

### Neutral
- **Ground** (#ffffff light, #0f0f11 dark): the page background, from the OS theme.
- **Surface** (#f6f6f4 light, #1a1a1d dark): the log panel and banner fills.
- **Ink** (#161616 light, #f0f0f1 dark): primary text.
- **Muted** (#5f5f66 light, #9c9ca4 dark): secondary text — step messages, config keys, section titles.
- **Hairline** (#e5e5e2 light, #2e2e33 dark): all 1px rules and borders.

### Signal
- **Signal Green** (#0b7c4e light, #3ccb8b dark): done states and the "started" status.
- **Signal Amber** (#a8620a light, #e2a03a dark): warnings — crash banner, config errors, skipped steps.
- **Signal Red** (#c6352e light, #f06a6a dark): failures — error banner, stale-process banner, danger buttons.
- **Fill Ink** (#ffffff light, #0f0f11 dark): the check/cross drawn on signal fills — white on the deep light-mode greens/reds, dark ink on the bright dark-mode ones.

### Named Rules
**The One Accent Rule.** Ultramarine is the only hue. Green, amber, and red exist solely as status signals; any other chromatic color breaks the instrument.

**The Theme Rule.** Light and dark are both designed, never inherited. The page follows the OS through `prefers-color-scheme`; it never offers its own toggle.

## Typography

**Display/UI Font:** Inter (self-hosted, served offline via the vfs handler)
**Body Font:** Inter 500
**Data/Mono Font:** JetBrains Mono / Cascadia Code / Consolas

**Character:** A neutral grotesque — the Swiss program voice — set on a strict scale with tracked uppercase labels. Type is measured, never decorative; Chinese text falls back through the stack to the platform CJK face.

### Hierarchy
- **Display** (600, 24px, tracking 0.02em): the DSHL wordmark.
- **Title** (600, 11px, uppercase, tracking 0.12em): section labels (启动流程 / 配置 / 日志).
- **Body** (500, 14px, line-height 1.35): step titles.
- **Label** (600, 12px, uppercase, tracking 0.06em): buttons; the status stamp is 11px at 0.08em.
- **Data** (400, 12px, mono, line-height 1.65): log lines, config keys/values, paths — tabular numerals throughout.

### Named Rules
**The Mono-For-Data Rule.** Monospace appears only where code or machine output is shown — the log. Everything else speaks in the grotesque; mono headings would be costume.

## Layout

A centered shell (max-width 980px) on a strict vertical rhythm: header row (brand left, status stamp right) under a hairline; two ruled columns below (启动流程 checkbox / 配置 kv) with a 48px gutter; the terminal log spans full width beneath; a fixed bottom bar holds the actions under a hairline. Spacing rhythm in px: shell padding 28×28 (mobile 24×20), header padding-bottom 16, main 28 below the header, section labels 16 above their content, steps rows 11, kv rows 7, log-card 28 above its label, footer padding 16×28. The shell's bottom padding is the action bar's measured height plus 24px breathing room (`--footer-h`, measured by app.js as the bar wraps) — never a fixed placeholder, so short landscape screens don't grow an unnecessary page scrollbar. The rhythm is tuned so a landscape viewport (≥768px tall) holds the whole page without scrolling; shorter viewports scroll. At ≤720px the columns stack, the header stacks, and the footer wraps.

## Elevation & Depth

Flat by declaration. There are no shadows and no elevation levels — depth comes from the whitespace between ruled blocks and from the hairline against the ground. A pressed button yields 1px of travel and a color shift; that is the only "lift" in the system.

## Shapes

Square, always: 0px radius. Every border is a 1px hairline. Status is carried by 8px color squares (the status stamp LED) and by row numbers tinted with the status color. Nothing is a pill, nothing is clipped, nothing casts a shadow.

## Components

### Buttons
- **Shape:** square (0px radius), 1px border, 12px uppercase tracked label.
- **Default:** transparent face, ink text, hairline border. Hover fills the surface and darkens the border to ink; press moves 1px.
- **Primary (jump):** ultramarine face, white text. Hover/press darken to ultramarine-strong.
- **Danger:** transparent face, red text and border. Hover fills red with white text.
- **Focus:** 2px ultramarine outline, 2px offset.

### Status Stamp
A small uppercase note (11px, 0.08em) with an 8px currentColor square. Its color is driven by the runtime state via CSS variables, so it adapts to both themes.

### Steps (checklist)
Rows on hairline separators: a double-layer square indicator on the left,
title and message on the right. The rows are read top-down in install order,
so no numerals are needed. States: **pending** — two empty nested square
outlines; **running** — the inner core swells and pales (size + colour)
inside the static ultramarine ring; **done** —
filled signal-green square with a check (a rotated L border, no glyph) in the
theme's fill-ink; **error** — filled signal-red square with a cross in the
fill-ink; **skipped** — dimmed hairline fill with the title struck through.

### Config
A two-column ruled table: muted mono keys and accent-soft mono values (tabular), separated by hairlines. Values break-all so long paths and URLs never overflow.

### Terminal Log
A surface panel (1px hairline border, mono 12px, line-height 1.65, max-height 185px) with a pulsing ultramarine block cursor at the end. The log follows the user: while they sit at the bottom, new entries scroll it to the very end; the moment they scroll up to read history, it stays put and never yanks.

### Motion
Three quiet behaviors, all reserved for live states, nothing decorative:
- **Log cursor** — an ultramarine block pulses at the end of the log (1.1s `steps(2,start)`).
- **Running step** — one modest swell: the core rises over the first third of the cycle (0.9× → 1.5×, accent → faint), then eases back. Speed is 1.6× the log cursor's blink (cursor 1.1s → core ≈0.69s ease-in-out).
- **Status LED** — the header's colour square breathes (2s ease-in-out) only while `body.running` (the launch is in progress); done/error/crash states sit still.

### Scrollbars
Thin and quiet on both the page and the log: a 6px thumb in the hairline tone on a transparent track, waking to the muted tone on hover. Chromium `scrollbar-width/color` plus the webkit pseudo-elements; no visible track chrome.

## Do's and Don'ts

### Do:
- **Do** build structure with 1px hairlines and whitespace; the grid is the design.
- **Do** spend ultramarine only on the primary action, the running state, and key values.
- **Do** author both light and dark grounds and let the OS choose.
- **Do** use square corners, tabular numerals, and tracked uppercase labels.
- **Do** keep the self-hosted Inter as the voice; reserve mono for the log.
- **Do** let the log follow the user: pinned to the end until they scroll up.

### Don't:
- **Don't** use shadows, gradients, rounded corners, or glows.
- **Don't** add a second accent hue; keep green/amber/red purely functional.
- **Don't** use a system sans as the display voice.
- **Don't** use glyph icons or emoji; carry status with the double-layer square, checks, and crosses.
- **Don't** reserve a fixed placeholder for the action bar; measure its real height.
- **Don't** add motion beyond the three live signals — the log cursor, the running step's swelling inner core, and the status LED while launching.