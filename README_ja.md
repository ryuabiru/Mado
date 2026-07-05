# Mado

[English README](README.md)

Mado はミニマルな Neovim 向け GUI クライアントです。Neovim を embedded mode で起動し、
line-grid UI をネイティブの GPU 描画ウィンドウに反映し、キーボード・マウス・IME の確定入力を Neovim に渡します。

つまり、ターミナルから `nvim` を開く代わりに、アプリをダブルクリックして Neovim を起動できる、というイメージです。

特に IME 対応を重視していて、日本語入力をターミナルの回避策に頼らずネイティブウィンドウ上で扱えます。変換中の
preedit 表示や確定入力も考慮されているので、日本語を多く使う環境でも扱いやすい構成です。

技術的な言い方をすると、Mado は Neovim を内部で起動して、その画面表示をアプリのウィンドウに描画します。ですが、
使う側の感覚としては「Neovim をアプリとして開ける」「日本語入力しやすい」という理解で大丈夫です。

## ステータス

Mado はオープンソースで、MIT ライセンスで公開されています。

最初の公開リリース対象は次の環境です。

- Apple Silicon 搭載の macOS 14 以降（M1 / M2 / M3 / M4）

Windows 向けの実装もリポジトリには含まれていますが、現時点では公開済みの配布物の主対象ではありません。

## 動作要件

通常の利用だけなら、まずは公開リリースのアプリを開けば使えます。ここは主に開発者向けの要件です。

- Rust 1.85 以降
- `ext_linegrid` をサポートする Neovim

Mado は通常、Neovim を自動で探します。技術的には `MADO_NVIM`、`PATH`、および macOS / Windows の一般的な
インストール先を確認します。

## リリース版の導入

最初の GitHub リリースでは、macOS Apple Silicon 向けビルドをダウンロードして `Mado.app` を開いてください。
初回起動時に macOS の警告が出ることがあります。その場合は、ダウンロード元を確認したうえで Finder から
**Open** を使って開いてください。

## 設定の考え方

Neovim の配色、ハイライト、カーソル形状は Neovim 側が管理します。Mado が担当するのはフォントや初期ウィンドウサイズ
のようなアプリウィンドウまわりです。

Neovim をカスタマイズしている人は、たとえば `init.lua` に次のように書けます。

```lua
vim.cmd.colorscheme("catppuccin-mocha")
vim.o.guicursor = "n-v-c:block,i-ci-ve:ver25,r-cr-o:hor20"
```

## 設定ファイル

Mado の見た目や初期ウィンドウサイズは、設定ファイル `config.toml` で変更できます。

Mado は `config.toml` を次の場所から読み込みます。

- macOS: `~/Library/Application Support/Mado/config.toml`
- Windows: `%APPDATA%\\Mado\\config.toml`

すべての項目は省略可能です。組み込みのデフォルト値は次の内容と同等です。

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

### 設定できる項目

#### `font`

- `font.family`
  - エディタ表示に使うフォントファミリー名です。
  - 既定値は `HackGen Console NF` です。
  - コード表示、Nerd Font のグリフ、日本語表示の相性を考えた初期値になっています。

- `font.size`
  - フォントサイズです。
  - 指定できる範囲は `6.0` から `72.0` です。
  - 既定値は `15.0` です。

#### `window`

- `window.width`
  - 初回起動時のウィンドウ幅です。
  - 指定できる範囲は `320` から `16384` です。
  - 既定値は `960` です。

- `window.height`
  - 初回起動時のウィンドウ高さです。
  - 指定できる範囲は `200` から `16384` です。
  - 既定値は `640` です。

- `window.theme`
  - ネイティブウィンドウの外観テーマです。
  - 選択肢は次の 3 つです。
    - `auto`: OS の設定に合わせます
    - `light`: 常にライトテーマにします
    - `dark`: 常にダークテーマにします
  - 既定値は `auto` です。

- `window.opacity`
  - ウィンドウ背景の透明度です。
  - 指定できる範囲は `0.05` から `1.0` です。
  - `1.0` は完全不透明、`1.0` より小さい値は半透明です。
  - Neovim 側に別のカラースキームを用意しなくても、背景を少し透過させたいときに使えます。
  - 既定値は `1.0` です。

- `window.blur`
  - `true` / `false` のブール値です。
  - `true` にすると、対応している環境では透明部分に OS のぼかし効果を使います。
  - 既定値は `false` です。

### 設定例

```toml
[font]
family = "HackGen Console NF"
size = 16.0

[window]
width = 1200
height = 800
theme = "dark"
opacity = 0.9
blur = true
```

### 設定ファイルの読み込みルール

- 項目を省略した場合、その項目だけ既定値が使われます。
- `mado --config PATH` を使うと、別の設定ファイルを読み込めます。
- 未知のキーや不正な値が含まれる場合は、安全な既定値にフォールバックします。
- Mado は独自のエディタ用カラースキームを持たず、配色は Neovim 側の設定を使います。

macOS アプリのメニューには **Settings...** があり、`config.toml` を開きます。まだ存在しない場合は自動で作成されます。

## 実行

普段はアプリとして起動すれば十分です。ここでは開発中にコマンドから起動する例を示します。

```sh
cargo run -- README.md
```

通常の Neovim と同じように使い、終了は `:qa!` です。未保存のバッファがある状態でウィンドウを閉じると、Neovim 側の確認に従います。
`RUST_LOG=mado=debug` を設定すると、未知の UI イベントやプロトコル診断も含めて確認できます。

## テスト

この節は主に開発者向けです。

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

テストには画面描画、入力、IME、RPC に加えて、Neovim がインストールされていれば実際の
`nvim --clean --embed` 接続テストも含まれます。

## macOS アプリとファイル関連付け

開発者向けには、PowerShell から署名付きのアプリバンドルをビルドできます。

```powershell
./scripts/build-macos-app.ps1
open ./target/release/Mado.app
```

常用する場合は `Mado.app` を `/Applications` にコピーしてください。Finder ではソースファイルを control-click して
**Open With → Mado** を選ぶか、ファイル情報から既定アプリとして設定できます。
Finder からのファイルオープンは、未保存の作業を破棄せず、実行中の Neovim インスタンスに転送されます。

## Windows アプリとファイル関連付け

Mado をビルドして、現在の Windows ユーザー向けに関連付けを登録できます。

```powershell
cargo build --release
./packaging/windows/install-associations.ps1
```

Mado は一般的なテキストファイルやソースコードファイルの **Open with** に表示されます。既定アプリの最終的な選択は
Windows Settings 側が担当します。登録解除は `./packaging/windows/uninstall-associations.ps1` です。
開発者向けには、WiX v4 向けのインストーラー定義が `packaging/windows/Mado.wxs` にあります。
