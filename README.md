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
| `soltool` | Asset extraction and theme conversion CLI. |
| `sol-shell` | Minimal winit dev shell to run presenter + renderer before real frontends. |
| `sol-qt` | Linux frontend — Qt6/QML chrome via cxx-qt, wgpu playfield. |
| `sol-win32` | Windows frontend — native-windows-gui chrome, Win32 menu bar, wgpu playfield. |
