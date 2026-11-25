# Design — Postlite

A locked design system for this app. Every visual change reads this file before
touching `ui/`. Do not regenerate per view — extend or amend this file when the
system needs to grow.

## Genre

modern-minimal — a dense, functional developer tool. Restraint carries the
design: hairline rules, flat surfaces, weight-based hierarchy, and one warm
signal color.

## Theme

OKLCH throughout. Warm graphite surfaces (hue ~70, very low chroma) so the ember
accent sits natively; light theme is warm paper. Values live in `ui/styles.css`
`:root` and `:root[data-theme="light"]`.

| Token | Dark (default) | Light |
| --- | --- | --- |
| `--bg-0` (window / stage) | `oklch(16% 0.008 70)` | `oklch(96.5% 0.006 85)` |
| `--bg-1` (main panel) | `oklch(19% 0.01 70)` | `oklch(99% 0.004 85)` |
| `--bg-2` (raised) | `oklch(23% 0.012 70)` | `oklch(95.5% 0.008 85)` |
| `--bg-3` (hover) | `oklch(28% 0.014 70)` | `oklch(92.5% 0.01 85)` |
| `--line` | `oklch(31% 0.015 70)` | `oklch(87% 0.01 85)` |
| `--fg` | `oklch(93% 0.008 80)` | `oklch(26% 0.02 70)` |
| `--accent` (ember) | `oklch(72% 0.17 42)` | `oklch(52% 0.16 40)` |

Semantic hues are fixed by HTTP meaning (methods, statuses). Only their values
are tuned; hue families never change: GET green, POST ink, PUT blue, PATCH
violet, DELETE red, OPTIONS amber, QUERY cyan; 2xx green, 3xx blue, 4xx amber,
5xx red.

## Accent discipline

Ember appears in ≤ 5% of any viewport: the Send button (solid fill), active
markers (tab underline, active tree row edge), focus rings, and the brand mark.
Everything else is ink-on-graphite. The eight settings presets (ember, ocean,
mint, violet, rose, amber, teal, magenta) stay — each has tuned dark and light
variants. Method/status hues are deliberately untouched by presets.

## Typography

- Sans: system stack (`Segoe UI Variable Text`, Inter, system-ui) — no webfonts,
  this is a native desktop app.
- Mono: `Cascadia Code`, JetBrains Mono, Consolas, ui-monospace — URLs, method
  chips, code, values.
- Hierarchy comes from weight (650/600/500) and size (13/12/11px), never from
  boxes or borders. `font-variant-numeric: tabular-nums` on all metrics.
- No italic outside `.j-null` and code comments. No letterspaced uppercase
  kickers.

## Surfaces & elevation

Flat. Gradients are banned on structural surfaces (topbar, dialogs, drawers,
request pane). Elevation is one soft shadow on popovers/dialogs
(`--shadow-pop`) and one edge shadow on the drawer (`--shadow-edge`); no glow
shadows on UI state except the save-dot and drag line. Separation is done with
1px hairlines (`--line` / `--line-soft`), not stacked backgrounds.

## Spacing & shape

- 4-pt rhythm; radii: `--r-s` 6px (rows, inputs, small controls), `--r-m` 8px
  (buttons, panels, urlbox), `--r-l` 12px (dialogs, overlays). Pills 999px.
- Controls: request row height 26px; tab strip 38px; topbar 44px; statusbar 26px.

## Motion

- `--t-fast` 140ms for hovers/state; `--t-med` 200ms for panes/dialogs.
- One easing: `--ease: cubic-bezier(0.2, 0.7, 0.3, 1)`. No bounce.
- `prefers-reduced-motion: reduce` collapses everything (already in place).

## Microinteractions stance

- Silent success; no celebratory toasts.
- Focus is instant: `:focus-visible` outline 2px accent, `--ring` accent-tinted,
  never animated.
- Hover states are background shifts, never color-only.

## CTA voice

- Primary: **solid accent fill**, dark ink text, 8px radius, 700 weight (Send,
  dialog confirm).
- Secondary: outlined on `--bg-2` (`.tool.outlined`).
- Ghost: transparent, hover `--bg-3` (`.tool`, `.icon-btn`).
- Destructive: flat solid red fill (no gradient, no glow).
- Toggles/checkboxes are neutral state, not accent action: filled `--fg`, never
  orange.

## What views MUST share

- The token names in `ui/styles.css` (the app JS and both themes depend on the
  `data-theme` / `data-accent` contract — never rename or remove a token).
- The accent placement rules and the fixed semantic hue families.
- The CTA voices, radii, and control heights.
- The system font stacks.

## What views MAY differ on

- Background band choice (`--bg-0` vs `--bg-1`) per stage: request pane sits on
  `--bg-1`, response stage on `--bg-0` — this is the only intentional contrast.
- Empty states may use the faint dot grid (`--dot`).

## Exports

### tokens.css (mirror of `ui/styles.css` `:root`)
```css
:root {
  --accent: oklch(72% 0.17 42);      /* ember, dark */
  --accent-hi: oklch(79% 0.15 48);
  --ok: oklch(75% 0.16 155);  --warn: oklch(78% 0.14 75);
  --err: oklch(66% 0.19 15);  --info: oklch(70% 0.13 240);
  --cyan: oklch(78% 0.11 210); --violet: oklch(74% 0.13 300);
}
```
(Theme-specific and per-preset values live in `ui/styles.css`; keep the two
files in sync when amending.)

### shadcn/ui variables
```css
:root { --background: 19% 0.01 70; --foreground: 93% 0.008 80;
        --primary: 72% 0.17 42; --border: 31% 0.015 70;
        --ring: 72% 0.17 42; --radius: 0.5rem; }
```
