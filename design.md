# Design — Postlite desktop

The desktop app is a workbench for sending and inspecting API requests. Its visual system carries the website's warm paper, graphite, and ember into a compact native interface.

## Workbench

- Put the workspace and its active environment beside the Postlite mark. Keep global actions at the opposite edge.
- Show collections, environments, and history in the shared rail and resizable sidebar. Mark the selected request in both the collection tree and tab strip.
- Keep one address bar above the request and response panes. Method, URL, and Send form the main workflow. Save and the layout control sit to their right.
- Stack panels by default. At 840 px of usable workbench width, offer a side-by-side view. When space shrinks, use the stacked layout and restore the user's choice when space returns. Save the two separator sizes independently.
- Give each pane a clear heading and move Cookies and Code into the request header. Keep response status, time, and size together before response actions.
- Use the empty workspace to help someone open, create, or import a request. Use a quiet icon and keyboard hint for a response that has yet to arrive.

## Palette

The default accent is ember. The eight accent presets change action and focus colors. Request methods, response statuses, and syntax retain consistent semantic colors.

| Token | Dark | Light |
| --- | --- | --- |
| `--bg-0` response and rail | `#1b1815` | `#f8f4eb` |
| `--bg-1` request paper | `#251f1b` | `#fffaf1` |
| `--bg-2` raised controls | `#2e2721` | `#f0e9de` |
| `--bg-3` hover | `#3a3029` | `#e8ddcf` |
| `--bg-side` sidebar and chrome | `#211c18` | `#f1ebe0` |
| `--line` | `#483c33` | `#cfc1b2` |
| `--fg` | `#fff3e5` | `#261c16` |
| `--accent` ember | `#ff8964` | `#ae3218` |

`ui/styles.css` owns the tokens. Theme, accent, and workbench style use `data-theme`, `data-accent`, and `data-appearance` on the root element.

## Appearances

The Workbench style setting is independent of light and dark mode and the accent. The app remembers it with the other preferences and applies it without switching the active request or response view.

| Style | Structure |
| --- | --- |
| Workbench | Warm surfaces, balanced spacing, and the original workbench controls |
| Terminal | Compact chrome, monospace labels, square controls, and striped parameter rows |
| Studio | Rounded panes and controls, pill-shaped tabs, and larger headings with more space |
| Ledger | Serif headings, divided request tabs, ruled navigation rows, and a response gutter |

## Type and movement

- Use the system sans stack for controls and headings and the mono stack for URLs, code, methods, and metrics. Status metrics use tabular numbers.
- Use weight, surface contrast, and short rules for hierarchy. The Send button has a solid accent fill. The selected tab and focused controls use the active accent.
- Keep the shell flat. Use shadows on the floating popovers and drawers. The empty workspace can use the quiet dot pattern.
- Keep hover and pane transitions at 140 ms and 200 ms. Respect the system reduced-motion preference and expose both resize handles to keyboard users.
