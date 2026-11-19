# Scriptura アイコン

OpenAI Image API（`gpt-image-2.6-sunburst`）の参照編集で生成した、藍のガラスの文書に金のペン先を重ね、琥珀色の更新バッジを添えたアイコンです。作者の Chrome 拡張機能（SearchClock・EvidenceShot・Clean Reload）のアイコンを参照画像にして、光沢のあるガラス調の画風をそろえています。背景は透過です。生成時の全文プロンプトは [prompt.txt](prompt.txt) に保存しています。

| ファイル | 用途 |
| --- | --- |
| `scriptura.png` | 生成マスター、1024×1024、透過RGBA |
| `icon-16.png` ～ `icon-512.png` | 各表示サイズ用の透過PNG（32px 以下は余白を詰めた版） |
| `scriptura.ico` | 16・20・24・32・48・64・128・256px 入り（すべて 32bpp の BMP 形式）。Windows の実行ファイル・タスクトレイ・Velopack の Setup 向け |
| `scriptura.icns` | macOS のアプリアイコン（Velopack の `.app` / `.pkg`）向け |

Pillow で余白をそろえ（生成時の薄いもやはアルファ 24 以下を透明化）、リサイズと ICO・ICNS への形式変換をしただけで、図柄は加工していません。

- Windows: `build.rs` が `scriptura.ico` を `RT_ICON` / `RT_GROUP_ICON` のリソースとして `scriptura.exe` に埋め込みます（Explorer・スタートメニュー・サインイン時の起動登録の表示用）。タスクトレイ（`src/ui/tray/windows.rs`）は同じ ICO を `include_bytes!` で持ち、小アイコンの寸法に最も近い画像を読みます。
- macOS: メニューバー（`src/ui/tray/macos.rs`）は `icon-32.png` を `include_bytes!` で持ち、18pt で表示します。
- 配信: `scripts/velopack/pack-windows.ps1` は `scriptura.ico`、`pack-macos.sh` は `scriptura.icns`、`pack-linux.sh` は `icon-256.png`（AppImage）を `vpk pack --icon` に渡します。
