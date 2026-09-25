<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="100%">
</p>

<p align="center"><b>AI ネイティブな開発のためのオーケストレーション・ターミナル</b></p>

<p align="center">
  <a href="https://www.agentty.run">agentty.run</a> ·
  <a href="https://www.agentty.run/docs">ドキュメント</a> ·
  <a href="https://github.com/empty-user77/Agentty/releases">ダウンロード</a> ·
  <a href="https://x.com/agentty_run">@agentty_run</a>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.ko.md">한국어</a> ·
  <b>日本語</b> ·
  <a href="README.zh.md">中文</a>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="Platform: macOS | Windows | Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey.svg">
</p>

Agentty は、**Claude Code**、**Codex**、そしてシェルを一つのウィンドウで並べて動かすネイティブな GPU レンダリング
ターミナルです。どのエージェントが作業中で、どれが終わり、どれが返事を待っているのかを示し、それぞれに作業で必要な
もの — プロジェクトのファイル、Git の履歴、コンテナ、データベース — を渡します。

Rust と [GPUI](https://gpui.rs)、`alacritty_terminal` で作られています。Electron は使っていません。

## Agentty を使う理由

- **すべてのエージェントを一つのウィンドウに** — ワークスペース・タブ・分割ペインは再起動後も復元されます。
  ターミナル、Claude Code、Codex のほか、Gemini、Copilot、Cursor、OpenCode、Amp、Aider、ローカルの Ollama モデルも
  同じメニューから起動できます。
- **状態を見分けられます** — ペインとワークスペースごとに、作業中・完了・入力待ちをリアルタイムで表示します。返事が
  必要なペインは縁が光り、通知はデスクトップとメニューバーに、離席中なら Slack・Discord・Telegram にも届きます。
- **セッション間で文脈を渡せます** — 二つのエージェントを線でつないで会話を引き継ぎ、双方向で同期し続けたり、
  Claude Code と Codex の間でセッションを移行できます。
- **衝突しない並列作業** — 同じプロジェクトで二つ目のエージェントを、新しいブランチの専用 git ワークツリーで始めるか既存のものを共有するか尋ね、
  エージェントが作業を複数セッションに分けたいと求めたときは、確認のうえ一度に開始します。
- **ターミナル以上のもの** — エディタ付きのファイルパネル、GitHub Desktop 風の Git ページ、Docker パネル、
  データベースページ、アプリ内ブラウザ、使用量とコストのモニタリング、この PC のすべての git ワークツリーの管理（未コミット・マージ状況の確認、
  まとめて削除）、ビルド出力とツールキャッシュを片付けるディスク管理、スキル・サブエージェント・MCP サーバーを
  まとめる拡張ページ。
- **データの扱いは慎重に** — エージェントの参照は読み取り専用トランザクションの中で実行され、書き込みは実行する文を
  そのまま示す承認ダイアログを通らなければ実行されません。秘密情報は OS の資格情報ストアに保存され、会話履歴と
  プロンプトが端末を離れることはありません。

機能の詳しい説明は **[agentty.run/docs](https://www.agentty.run/docs)** にあります。

## インストール

### macOS（Apple シリコン、macOS 13 以降）

いちばん簡単なのは [Homebrew](https://brew.sh) です。

```sh
brew install --cask empty-user77/agentty/agentty
```

これだけで Agentty がアプリケーションフォルダに入ります。Homebrew がまだなら、[brew.sh](https://brew.sh) の 1 行の
コマンドで先にインストールしてから、上のコマンドを実行してください。

| | コマンド |
|---|---|
| アップデート | Agentty が自動で更新します。`brew upgrade --cask agentty` も使えます |
| アンインストール | `brew uninstall --cask agentty` |

直接ダウンロードする場合は、[最新リリース](https://github.com/empty-user77/Agentty/releases/latest)から `Agentty-X.Y.Z-…-arm64.dmg` を取得して開き、**Agentty** を
**アプリケーション**にドラッグします。Apple の署名と公証済みなので、警告なしで開きます。Intel Mac 向けのビルドはありません。

### Windows（Windows 10 1809 以降、x64）

1. [最新リリース](https://github.com/empty-user77/Agentty/releases/latest)から `Agentty-X.Y.Z-windows-x64-setup.exe` をダウンロードします。ブラウザが `.exe` を
   ブロックする場合は `Agentty-X.Y.Z-windows-x64-setup.zip` を取得して展開してください。同じインストーラーが入っています。
2. 実行します。ユーザー単位でインストールされ、管理者権限は不要です。
3. インストーラーはまだコード署名されていません。SmartScreen が表示されたら **詳細情報 → 実行** を選んでください。

アンインストール: 設定 → アプリ → Agentty → アンインストール。

### Linux（x86_64）

[最新リリース](https://github.com/empty-user77/Agentty/releases/latest)からディストリビューションに合うパッケージをダウンロードして:

```sh
# Debian 12+ / Ubuntu 22.04+
sudo apt install ./Agentty-*-linux-amd64.deb

# RHEL 9+ / Fedora
sudo dnf install ./Agentty-*-linux-x86_64.rpm
```

アンインストール: `sudo apt remove agentty` または `sudo dnf remove agentty`。

### アップデート

Agentty は起動時と 1 時間ごとに新しいバージョンを確認します。macOS と Windows ではアップデートをダウンロードし、
リリースのチェックサムで検証してからインストールして再起動します。Linux では新しいバージョンを知らせ、パッケージのページを案内します。

### 始める前に

Agentty はエージェントを動かしますが、エージェント自体は同梱していません。使う CLI をインストールし、`PATH` に
あることを確認してください。例: [Claude Code](https://docs.anthropic.com/en/docs/claude-code)（`claude`）、
[Codex](https://github.com/openai/codex)（`codex`）。Agentty の 設定 → システム から足りないものをインストールすることもできます。

### ソースからビルド

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

Rust 1.98（`rust-toolchain.toml` で固定）と各プラットフォームのビルドツールが必要です。Windows と Linux は
[docs/platforms.md](docs/platforms.md) を参照してください。

## ドキュメント

| | |
|---|---|
| [機能ガイド](https://www.agentty.run/docs) | アプリの各画面と機能の説明 |
| [docs/configuration.md](docs/configuration.md) | `settings.json`、エージェントのサインイン、ハーネス、データベース、テーマ、Agentty が作るファイル |
| [docs/platforms.md](docs/platforms.md) | Windows・Linux での違いとビルド方法 |
| [docs/plugins](docs/plugins/README.md) | プラグインの作り方（Node.js SDK、[プロトコル](docs/plugins/protocol.md)）と[使い方](docs/plugins/usage.md) |
| [docs/architecture.md](docs/architecture.md) | クレート・ターミナル・エージェントソケット・パネルの構成 |
| [docs/metrics.md](docs/metrics.md) | 公式ビルドが送りうるイベントの一覧と無効化の方法 |
| [docs/release.md](docs/release.md) | パッケージング・署名・公証とリリースチャネル |

## ショートカット

| 操作 | ショートカット |
|---|---|
| 新しいターミナル / Claude Code / Codex タブ | ⌘T / ⌥⌘C / ⌥⌘X |
| 右 / 下に分割 | ⌘D / ⇧⌘D |
| 次・前のペイン · タブ · ワークスペース | ⌘] ⌘[ · ⇧⌘] ⇧⌘[ · ⌥⌘↓ ⌥⌘↑ |
| コマンドパレット · 未読へ移動 | ⇧⌘P · ⇧⌘U |
| Git · セッション連携 · モニタリング · 設定 | ⇧⌘G · ⇧⌘F · ⌥⌘U · ⌘, |
| ファイルパネル · アプリ内ブラウザ · ミニモード | ⌥⌘B · ⇧⌘B · ⌃⌘M |

すべての一覧は **設定 → キーボードショートカット** にあります。Windows と Linux では ⌘ が Ctrl+Shift、⇧⌘ が
Ctrl+Alt+Shift、⌥⌘ が Ctrl+Alt になり、Ctrl+文字はシェルに渡ります。

## プラグイン

プラグインはターミナルの横のパネルを埋め、エージェントペインの上にボタンを、コマンドパレットに項目を追加し、
プロンプトを送れます — 送り先は必ず「送信先を選択」ダイアログで利用者が選びます。プラグインが求める権限はインストール前に
プラグインページで確認でき、プラグインが Agentty に頼める操作を制限します。WebAssembly プラグインはその権限の外には
何も触れられません。Node.js、Python またはネイティブプログラムとして動くプラグインは、Agentty の外ではあなたと同じ
ファイルとネットワークへのアクセス権を持つため、信頼できるものだけをインストールしてください。プラグインは
[Agentty Marketplace](https://github.com/empty-user77/Agentty-Marketplace) を通じて配布されます。標準添付の
**Cosmica** プラグインは [Cosmica](https://www.cosmica.ink/) のノートをプロンプトに変え、セッションの要約を書き戻します。

詳しくは [docs/plugins](docs/plugins/README.md) を参照してください。プラグインページから新規作成して Claude Code に
実装させることもできます。

## プライバシー

セッション一覧、使用量の計算、コンテキストの受け渡しのために、エージェントの会話履歴を **ローカルでのみ** 読みます。
プロンプト、出力、パス、リポジトリのデータを送信することはなく、エージェントの状態フックはアカウント専用の
ソケットとだけ通信します。

公式リリースビルドは匿名の利用統計（どの機能を使ったか、アプリとシステムのバージョン、ランダムなインストール ID）を
送ります。設定 → 一般 で無効にでき、`DO_NOT_TRACK=1` でも無効になります。ソースから
ビルドしたものは何も送りません。イベントの全一覧は [docs/metrics.md](docs/metrics.md) にあります。

## コントリビュート

貢献を歓迎します。[CONTRIBUTING.md](CONTRIBUTING.md) と[行動規範](CODE_OF_CONDUCT.md)をお読みください。
セキュリティに関する問題は [SECURITY.md](SECURITY.md) を参照してください。

## ライセンス

```
Copyright 2026 LEE YONGBEOM

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

   http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

全文は [LICENSE](LICENSE) にあります。

同梱フォントとサードパーティ構成要素はそれぞれのライセンスに従います —
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) を参照してください。
