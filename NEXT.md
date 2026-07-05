# Mado: remaining MVP work

The native window, line-grid rendering, keyboard and mouse forwarding, and IME
preedit/commit separation are implemented.

## Rendering refinements

- Benchmark the shaped-row cache with large grids and rapid scrolling.
- Add visual regression checks for Japanese fallback fonts and HiDPI displays.
- Decide whether undercurl, underline, and preedit indicators need snapshot-style rendering checks.
- Decide whether HiDPI pixel-snapping should extend from decorations to more text-adjacent overlays.

## Input and IME refinements

- Exercise Japanese IME manually on both macOS and Windows.
- Refine candidate-window behavior and edge cases across different IMEs.
- Decide whether long preedit should eventually wrap to a secondary inline row instead of staying single-line.

## Ergonomics refinements

- Expand compatibility testing against real-world Neovim plugin setups, especially LSP, lint, formatter, and completion stacks.
- Decide which embedded-UI capabilities are required to match day-to-day GUI expectations from mature clients such as Neovide.
- Audit Finder-launched macOS behavior beyond `PATH`, including project root detection and any plugin features that still assume a terminal-hosted session.
- Decide whether the window title should reflect the current file or modified state from Neovim.
- Decide whether Mado should expose a lightweight settings reload flow without requiring a full restart.
- Decide whether startup state should also include a minimal visual placeholder inside the window, not just a title change.

## Packaging refinements

- Add a production icon and signed/notarized macOS release pipeline.
- Build and smoke-test the WiX installer in Windows CI.
- Add release archives and checksums for both platforms.
