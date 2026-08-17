# classic-solitair

A Win98-faithful Klondike Solitaire, written in Rust as a Cargo workspace around a pure
event-sourced engine, wgpu rendering, and native Qt6/Win32 frontends. Licensed under
GPL-3.0-or-later.

> **Microsoft's original Solitaire artwork is never bundled with this project.** To play
> with the original look, run `soltool extract` against theme assets from your own
> licensed copy of Windows — the tool reads local files only; it never downloads or
> redistributes them.

## Workspace crates

| Crate | Role |
| --- | --- |
| `sol-engine` | Pure event-sourced Klondike rules; no I/O, no clock, no OS RNG (Gameplay context). |
| `sol-session` | Cross-game state — Vegas bankroll, options, versioned save/load (Session context). |
| `sol-theme` | Theme package format — manifest parsing, asset loading, validation. |
| `sol-presenter` | Layout, hit-testing, drag logic, animation timing → sprite display lists. |
| `sol-render-wgpu` | Batched wgpu sprite renderer for the playfield. |
| `soltool` | Asset extraction and theme authoring CLI. |
| `sol-shell` | Minimal winit dev shell to run presenter + renderer before real frontends. |
| `sol-qt` | Linux frontend — Qt6/QML chrome via cxx-qt, wgpu playfield. |
| `sol-win32` | Windows frontend — native-windows-gui chrome, Win32 menu bar, wgpu playfield. |

## `soltool`

The asset extraction and theme authoring CLI. Subcommands:

- `extract <sol.exe | cards.dll | dir-of-bitmaps> -o <theme-dir> [--animate]` —
  pull card bitmaps from your own local Windows assets into a
  `render_mode = "png"` theme. **Output is for your local use only — the
  original artwork must never be redistributed or committed** (the tool reads
  local files only; it never downloads or ships them). `--animate`
  reconstructs the four animated card backs from the resource file's own
  overlay sprites; it takes a resource input only, and is a usage error on a
  loose directory, which already packs frame-numbered files into strips
  itself.
- `validate <theme>` — lint a theme package against the theme format rules;
  non-zero exit on failure.
- `pack-strip <frame files…> -o <strip.png> --fps <n>` — pack loose frames into
  one horizontal strip PNG and print a ready-to-paste `[backs]` snippet.

## Dev shell

`sol-shell` is the winit development shell: a fully playable game (deal,
draw, drag-and-drop, double-click to foundation, undo/redo, save/load,
win cascade) with keyboard shortcuts standing in for the real frontends'
menus. Run it with:

```sh
cargo run -p sol-shell                     # default theme, random deal
cargo run -p sol-shell -- --seed 42        # deal a specific game
cargo run -p sol-shell -- --theme <path>   # any theme dir or zip
```

`--help` (also printed at startup) lists the shortcuts: `F2` new deal,
`G <digits> Enter` select game by seed, `Ctrl+Z`/`Ctrl+Y` undo/redo,
`Ctrl+S`/`Ctrl+O` save/load, `D`/`M`/`T` draw mode/scoring/timed (next
deal), `O` outline dragging, `B` cycle card back, `Esc` quit. The board
fills the window: cards scale continuously with the window height and
the tableau columns spread across the width exactly as the original
did; windows narrower than the design aspect fill the width instead,
with felt below.

## Default theme

`themes/default/` is the in-tree default theme: 52 original vector card faces, a
static back, and a 2-frame animated back, all `render_mode = "vector"`. Every
asset is CC0-1.0 (see `themes/default/LICENSE`) — original geometric shapes with
no `<text>` elements (rank glyphs are drawn as paths, so rendering never depends
on fonts installed at raster time) and no copied deck art, so it ships in the
repo and is distributed freely, unlike the Win98 original theme above.

The whole package is produced by a deterministic, stdlib-only generator; nothing
under `themes/default/` is hand-edited except `LICENSE` and the generator
itself. Regenerate it with:

```sh
python3 themes/default/generate.py
```
