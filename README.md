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

- `extract <sol.exe | cards.dll | dir-of-bitmaps> -o <theme-dir>` — pull card
  bitmaps from your own local Windows assets into a `render_mode = "png"`
  theme. **Output is for your local use only — the original artwork must never
  be redistributed or committed** (the tool reads local files only; it never
  downloads or ships them).
- `validate <theme>` — lint a theme package against the theme format rules;
  non-zero exit on failure.
- `pack-strip <frame files…> -o <strip.png> --fps <n>` — pack loose frames into
  one horizontal strip PNG and print a ready-to-paste `[backs]` snippet.

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
