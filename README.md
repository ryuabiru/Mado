# Mado

Mado is a minimal Neovim GUI client. It starts Neovim in embedded mode, mirrors
its line-grid UI in a native GPU-rendered window, and forwards keyboard, mouse,
and committed IME input to Neovim.

## Requirements

- Rust 1.85 or newer
- Neovim with `ext_linegrid` support

Mado searches for Neovim through `MADO_NVIM`, `PATH`, and common macOS and
Windows install locations.

## Settings ownership

Neovim remains the source of truth for editor colors, highlights, and cursor
shape. Mado owns native-window concerns such as the font and initial window
size. For example, keep this in `init.lua`:

```lua
vim.cmd.colorscheme("catppuccin-mocha")
vim.o.guicursor = "n-v-c:block,i-ci-ve:ver25,r-cr-o:hor20"
```

Mado reads `config.toml` from:

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
```

Use `mado --config PATH` to load another file. Missing or invalid settings fall
back to safe defaults. Mado does not maintain a separate editor colorscheme.

## Run

```sh
cargo run -- README.md
```

Use Mado like Neovim and exit with `:qa!`. Closing the window asks Neovim to
confirm when buffers contain unsaved changes. Set `RUST_LOG=mado=debug` to
include unknown UI events and protocol diagnostics.

## Test

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

The test suite includes grid, input, IME, RPC, and a real
`nvim --clean --embed` attach test when Neovim is installed.

## macOS application and file associations

Build an ad-hoc signed application bundle from PowerShell:

```powershell
./scripts/build-macos-app.ps1
open ./target/release/Mado.app
```

To keep it, copy `Mado.app` to `/Applications`. In Finder, control-click a
source file, choose **Open With → Mado**, or use **Get Info → Open with →
Change All** to make it the default for that file type. Finder file-open events
are forwarded to the running Neovim instance without replacing unsaved work.

## Windows application and file associations

Build Mado and register it for the current Windows user:

```powershell
cargo build --release
./packaging/windows/install-associations.ps1
```

Mado will appear under **Open with** for common text and source-code files.
Windows Settings remains responsible for selecting the default application.
Undo the registration with `./packaging/windows/uninstall-associations.ps1`.
A WiX v4 installer source is also available at `packaging/windows/Mado.wxs`.
