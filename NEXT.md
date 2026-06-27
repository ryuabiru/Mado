# Mado: remaining MVP work

The native window, line-grid rendering, keyboard and mouse forwarding, and IME
preedit/commit separation are implemented.

## Rendering refinements

- Cache shaped rows instead of rebuilding glyph buffers after every flush.
- Render Neovim's mode-specific cursor shape and blink timing.
- Add visual regression checks for Japanese fallback fonts and HiDPI displays.

## Input and IME refinements

- Exercise Japanese IME manually on both macOS and Windows.
- Refine long preedit wrapping and selection-range display.
- Add horizontal high-resolution wheel accumulation.

## Packaging refinements

- Add a production icon and signed/notarized macOS release pipeline.
- Build and smoke-test the WiX installer in Windows CI.
- Add release archives and checksums for both platforms.
