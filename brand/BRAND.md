# Roomy brand (v2)

Roomy should feel like a well-made object: calm, precise, quietly confident. Warm paper and near-black
ink carry almost everything. Color comes from a small, deliberate set of soft pastels that each have a
job, and one signature, **butter**, marks the room you just made.

Files in this folder:

| File | Use |
| --- | --- |
| `logo.svg` | Glyph + wordmark lockup. Ink switches to `#F2F0EA` under `prefers-color-scheme: dark`. |
| `glyph.svg` | The three-cell glyph alone, for inline use beside text or in tight UI. |
| `mark.svg` | App icon: ink tile, paper cells, one butter cell. 512 grid, 22.4% corner. Source for all icon sizes. |
| `mark-1024.png` | 1024px raster of `mark.svg`, for icon generators. |
| `tokens.json` | Source of truth for every value below. |
| `tokens.css` | The same tokens as CSS custom properties (light default, `[data-theme="dark"]`). |

## The idea

**Make room.** The glyph is the smallest possible treemap: three cells of a drive, one of them freed.
The freed cell is butter. The same move repeats everywhere: in the product, the one butter tile in a
treemap is the space Roomy just gave back.

## Signature: butter

`#F6D374` (dark `#F2D27A`). A soft, warm yellow, like a highlighter on paper.

*Why butter:* no disk cleaner or storage tool owns soft yellow (they're blue, green, magenta, or
rainbow-on-black), and yellow is the color of a cleared space, so it can mean "room made" without
a word.

Rules:
- Use it for **the primary action** (filled button, ink label) and for **the highlight**: the freed
  tile, the current selection, the one number that matters. One butter thing per view, ideally.
- Never set text in butter on paper. For butter-colored text or icons on light surfaces use
  `butter-deep #7A5A0C`. In dark mode butter itself is readable (13:1 on paper-dark).
- Never use butter as a data category. It is reserved for "this is the room you made".

## Neutrals

| Token | Light | Dark | Role |
| --- | --- | --- | --- |
| paper | `#F4F2EC` | `#131210` | Page and window background |
| surface | `#FBFAF6` | `#1C1B18` | Raised panels, windows, popovers |
| sunken | `#EAE7DF` | `#0E0D0B` | Wells, fields, code, tracks |
| line | `#DEDAD0` | `#2A2824` | Hairlines and dividers |
| line-strong | `#C9C3B7` | `#3D3A34` | Control borders, emphasis rules |
| ink | `#1B1A17` | `#F2F0EA` | Primary text, icons, secondary buttons |
| ink-2 | `#5C584F` | `#A7A296` | Secondary text |
| ink-3 | `#6A655B` | `#8B867B` | Tertiary text, captions (still AA) |

Light is the default theme. Dark is a deliberate warm near-black, not blue-black, so the pastels keep
their temperature.

## Status

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| danger | `#B5382A` (white text) | `#E87A6A` (ink text) | Permanent delete, destructive confirms. Never decorative. |
| danger-wash | `#F7DCD6` | `#3A201C` | Background for warnings |
| positive | `#466A3D` | `#B1C2A6` | "Done" confirmations, sparingly |
| focus | `#1B1A17` | `#F2D27A` | 2px focus ring, 2px offset |

Danger is a clear brick red, far from butter, so "make room" and "delete forever" never look alike.

## Data visualization

Twelve pastels with matched deep tones. Fills always carry **ink labels** (`#1B1A17`, at least 7:1 on
every fill in both themes). Use the **deep** tone of the same hue for legend text, icons and numbers on
paper (at least 5.4:1). Dark mode uses slightly deeper fills so nothing glares on near-black.

| Name | Light fill | Dark fill | Deep (text on light) |
| --- | --- | --- | --- |
| powder | `#C8D5E6` | `#A9B9CF` | `#3E5A80` |
| sage | `#CEDDC5` | `#B1C2A6` | `#466A3D` |
| lilac | `#D8CFE8` | `#BBB0CF` | `#5D4C86` |
| peach | `#F3D6BF` | `#D9B79E` | `#8E4F26` |
| mist | `#C6DDDF` | `#A7C1C3` | `#2F6A70` |
| blush | `#F0D0CC` | `#D6B0AB` | `#9A3F37` |
| pistachio | `#E0E6BF` | `#C3CAA0` | `#5B6620` |
| sand | `#E6DAC3` | `#CBBEA5` | `#74602F` |
| mauve | `#E5CEDC` | `#C9AFBF` | `#80456A` |
| cocoa | `#DCC9BC` | `#C1AD9F` | `#6E5243` |
| slate | `#BFC3C9` | `#A3A8AF` | `#4E5560` |
| stone | `#D8D3CA` | `#BDB7AC` | `#5C584F` |

In dark mode, the deep tone is the light fill (light text on dark).

**File-type categories** (app `Types` view and type bars):

| Category | Color |
| --- | --- |
| Video | powder |
| Images | sage |
| Audio | blush |
| Documents | sand |
| Archives | peach |
| Apps & code binaries | lilac |
| Code | mist |
| Virtual disks | cocoa |
| Game data | pistachio |
| Data & logs | mauve |
| System | slate |
| Hidden system space | stone with a 45° hatch (`viz.hatch`) |
| Other | stone |

**Treemap folders** cycle in `treemapOrder`: powder, sage, lilac, peach, mist, blush, pistachio, sand,
mauve, cocoa, slate, stone. Neighbors stay distinguishable. Separate tiles with a 2–3px gap of the
panel color rather than borders. The selected or freed tile is butter. Hidden or reserved space is always
shown, hatched, and labeled.

## Type

**Instrument Sans** (variable, `@fontsource-variable/instrument-sans`, import `standard.css` for
`wght` 400–700 and `wdth` 75–100). One family for everything. It's a crisp grotesk with a little more
character than the usual UI fonts: round, open bowls that echo the wordmark. Hierarchy comes from
size, weight and tracking, not from a second typeface. Use system mono for paths, sizes in tables
and commands.

| Style | Size | Weight | Line | Tracking |
| --- | --- | --- | --- | --- |
| Display | clamp(56px, 9vw, 120px) | 600 | 0.95 | -0.045em |
| Title 1 | clamp(36px, 5vw, 56px) | 600 | 1.04 | -0.035em |
| Title 2 | 28px | 600 | 1.15 | -0.025em |
| Title 3 | 20px | 600 | 1.3 | -0.012em |
| Body large | 19px | 400 | 1.55 | -0.005em |
| Body | 16px | 400 | 1.55 | 0 |
| UI | 13.5px | 500 | 1.35 | 0 |
| Caption | 12px | 500 | 1.35 | 0.01em |

Use tabular numbers (`font-variant-numeric: tabular-nums`) for sizes and counts. Use sentence case
everywhere, with no all-caps labels except tiny section kickers if unavoidable.

## Logo

- **Lockup**: glyph + lowercase "roomy" in Instrument Sans SemiBold, converted to outlines. The glyph is
  1.32× the x-height, centered on it, with a 0.36× x-height gap.
- **Clear space**: half the glyph's height on all sides.
- **Minimum size**: lockup 72px wide, glyph 12px, app icon 16px.
- **On color**: ink on paper, paper (`#F2F0EA`) on ink. The freed cell stays butter in both. Don't
  place the logo on pastels or photos, and don't recolor, outline, rotate, add effects, or retype it.

## App icon

`mark.svg`: ink tile, paper cells, butter cell. Export every platform size from it (Tauri:
`pnpm tauri icon ../brand/mark-1024.png` from `app/`). It's legible down to 16px. No gradients, gloss or shadows inside the
icon; the OS adds its own.

## Surfaces and shape

- Flat color only. **No decorative gradients, glows, glass/blur panels or neon.**
- Radii: tile 8, field 10, window 14, card 18, buttons pill (999).
- Depth: one soft window shadow (`shadow-window`) for floating windows and popovers. Everything else
  is flat with hairlines.
- Layout: generous whitespace, strong type, one idea per section. Avoid grids of identical cards.
- Icons: simple 1.75px line icons, only where they aid scanning. Never decorative "sparkles".

## Motion

`cubic-bezier(0.2, 0.7, 0.1, 1)`; 160ms for hovers, 240ms for state changes, 420ms for reveals. Motion
explains (a tile shrinking into freed space), never decorates. Always respect `prefers-reduced-motion`.

## Voice

Plain, warm, exact. Say what will happen and how to undo it. Don't promise speed numbers or perfection.
