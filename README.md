# Mado

[日本語版 README](README_ja.md)

Mado is a minimal Neovim GUI client. It starts Neovim in embedded mode, mirrors
its line-grid UI in a native GPU-rendered window, and forwards keyboard, mouse,
and committed IME input to Neovim. IME composition is supported so Japanese,
Chinese, and Korean text input can flow through a native window instead of a
terminal workaround.

In other words, instead of launching `nvim` from a terminal, you can launch
Neovim by opening an app window.

From a user's point of view, that mostly means "Neovim as a desktop app" with
better native window behavior and easier multilingual text input.

## Status

Mado is open source and published under the MIT license.

The first public release target is:

- macOS 14+ on Apple Silicon (M1, M2, M3, M4)

Windows support exists in the repository, but prebuilt public release artifacts
are not the focus yet.

## Requirements

If you only want to use a release build, you can usually just open the app.
This section is mostly for developers.

- Rust 1.85 or newer
- Neovim with `ext_linegrid` support

Mado normally tries to find Neovim automatically. Technically, it checks
`MADO_NVIM`, `PATH`, and common macOS and Windows install locations.

## Release install

For the first public GitHub release, download the macOS Apple Silicon build and
open `Mado.app`. On first launch, macOS may show a security warning. If that
happens, confirm the download source and open the app from Finder.

## Settings ownership

Neovim remains the source of truth for editor colors, highlights, and cursor
shape. Mado owns app-window concerns such as the font and initial window size.

If you already customize Neovim, you can keep settings like these in
`init.lua`:

```lua
vim.cmd.colorscheme("catppuccin-mocha")
vim.o.guicursor = "n-v-c:block,i-ci-ve:ver25,r-cr-o:hor20"
```

Mado reads its settings file, `config.toml`, from:

- macOS: `~/Library/Application Support/Mado/config.toml`
- Windows: `%APPDATA%\Mado\config.toml`

All fields are optional. The built-in defaults are equivalent to:

```toml
[font]
family = "HackGen Console NF"
size = 15.0

[window]
width = 960
height = 640
theme = "auto"
opacity = 1.0
blur = false
```

Available settings:

- `font.family`: font family name used for the editor window. The default
  `HackGen Console NF` is chosen to work well for code, Nerd Font glyphs, and
  Japanese text.
- `font.size`: font size from `6.0` to `72.0`.
- `window.width`: initial window width from `320` to `16384`.
- `window.height`: initial window height from `200` to `16384`.
- `window.theme`: native window appearance. Choose `auto`, `light`, or `dark`.
- `window.opacity`: background transparency from `0.05` to `1.0`.
- `window.blur`: `true` or `false`. When enabled, Mado asks the OS to blur
  transparent areas on platforms that support it.

If any setting file entry is missing, Mado fills it with the default value. If
the file includes unknown keys or invalid values, Mado falls back to safe
defaults.

Example:

```toml
[window]
theme = "dark"
opacity = 0.9
blur = true
```

Use `mado --config PATH` to load another file. Missing or invalid settings fall
back to safe defaults. Mado does not maintain a separate editor colorscheme.

## Run

For everyday use, opening the app is enough. This section shows a command-line
example for development.

```sh
cargo run -- README.md
```

Use Mado like Neovim and exit with `:qa!`. Closing the window asks Neovim to
confirm when buffers contain unsaved changes. Set `RUST_LOG=mado=debug` to
include unknown UI events and protocol diagnostics.

## Test

This section is mainly for developers.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

The test suite includes rendering-state, input, IME, RPC, and a real
`nvim --clean --embed` attach test when Neovim is installed.

## macOS application and file associations

For development, you can build an application bundle from PowerShell:

```powershell
./scripts/build-macos-app.ps1
open ./target/release/Mado.app
```

To keep it, copy `Mado.app` to `/Applications`. In Finder, control-click a
source file and choose **Open With → Mado**, or use the file info window to set
Mado as the default app for that file type. Finder file-open events are
forwarded to the running Neovim instance without replacing unsaved work.

The macOS app menu also includes **Settings...**, which opens Mado's
`config.toml` and creates it automatically if it does not exist yet.

## Windows application and file associations

Build Mado and register it for the current Windows user:

```powershell
cargo build --release
./packaging/windows/install-associations.ps1
```

Mado will appear under **Open with** for common text and source-code files.
Windows Settings remains responsible for selecting the default application.
Undo the registration with `./packaging/windows/uninstall-associations.ps1`.
For developers, a WiX v4 installer source is also available at
`packaging/windows/Mado.wxs`.
