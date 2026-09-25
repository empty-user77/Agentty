<h1 align="center">
  <a href="https://www.agentty.run"><img src="docs/assets/logo.png" alt="Agentty" width="64" valign="middle" /></a> Agentty
</h1>

<p align="center">
  <a href="https://github.com/empty-user77/Agentty"><img src="https://img.shields.io/github/stars/empty-user77/Agentty?style=flat&amp;label=%E2%98%85&amp;color=08C" alt="GitHub stars" /></a>
  <a href="https://github.com/empty-user77/Agentty/releases"><img src="https://img.shields.io/github/downloads/empty-user77/Agentty/total?style=flat&amp;color=08C" alt="ダウンロード数" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-08C?style=flat" alt="License: Apache-2.0" /></a>
  <a href="https://x.com/agentty_run"><img src="https://img.shields.io/badge/X-000000?logo=x&amp;logoColor=white" alt="Agentty を X でフォロー" /></a>
  <img src="https://img.shields.io/badge/macOS%20%7C%20Windows%20%7C%20Linux-4493F8?style=flat-square" alt="対応プラットフォーム: macOS、Windows、Linux" />
</p>

<p align="center">
  <sub><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <b>日本語</b> · <a href="README.zh.md">中文</a></sub>
</p>

<p align="center">
  <strong>AI ネイティブなワークフロー向けのオーケストレーション・ターミナル。</strong><br/>
  Claude Code、Codex、その他のエージェントを並べて実行 — 誰が作業中で、誰が終わり、誰が返事を待っているのかを一目で確認できます。
</p>

<h3 align="center"><a href="https://github.com/empty-user77/Agentty/releases/latest"><ins>Agentty をダウンロード</ins></a> · <a href="https://www.agentty.run/docs"><ins>ドキュメント</ins></a></h3>

<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="960" />
</p>

Rust と [GPUI](https://gpui.rs)、`alacritty_terminal` で作られたネイティブな GPU レンダリング・ターミナル。Electron は使っていません。
すべてのエージェントに作業で必要なもの — プロジェクトのファイル、Git の履歴、コンテナ、データベース — を渡します。

## 機能

<table>
<tr>
<td width="50%" valign="top">

### すべてのエージェントを一つのウィンドウに

ワークスペース、タブ、分割ペインでターミナル、Claude Code、Codex、その他のあらゆる CLI エージェントを管理 — 再起動後も復元されます。

[ドキュメント →](https://www.agentty.run/docs/workspaces-tabs-panes)

</td>
<td width="50%" valign="top">

### 誰が返事を待っているか見分けられる

各ペインとワークスペースに、エージェントの作業状況 — 作業中・完了・入力待ち — をリアルタイムで表示。デスクトップ、メニューバー、Slack、Discord、Telegram で通知を受け取れます。

[ドキュメント →](https://www.agentty.run/docs/agent-status)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### Session Flow

一つのエージェントから別のエージェントに線を引いてコンテキストを引き継ぎ、双方向で同期し続けたり、Claude Code と Codex の間でセッションを移行できます。

[ドキュメント →](https://www.agentty.run/docs/session-flow)

</td>
<td width="50%" valign="top">

### 並列ワークツリー

同じプロジェクトで二つ目のエージェントを、新しいブランチの専用 git ワークツリーで始めるか既存のものに加わるか選択できます。エージェントが複数セッションに作業を分けたいと求めたら、確認のうえ一度に開始します。

[ドキュメント →](https://www.agentty.run/docs/agent-git)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### AgentGit

リポジトリとブランチを切り替え、差分を確認、ファイル単位でコミット、履歴を閲覧、取得・プル・プッシュ・マージ — すべてエージェントを実行中のウィンドウから。

[ドキュメント →](https://www.agentty.run/docs/agent-git)

</td>
<td width="50%" valign="top">

### アプリ内ブラウザ

開発サーバーがターミナルの横に表示され、スマートフォンとタブレット・サイズにも対応。エージェントがページを操作してビルドしたものをテストできます。

[ドキュメント →](https://www.agentty.run/docs/in-app-browser)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### データベースと Docker

プロジェクトが既に設定しているデータベースを閲覧。エージェントは読み取り専用トランザクション内で読み取り、書き込みはすべて実行文を確認する承認ダイアログを通ります。コンテナ、ポート、ログはその横に。

[ドキュメント →](https://www.agentty.run/docs/databases)

</td>
<td width="50%" valign="top">

### プラグインとオートメーション

プラグインはパネルを追加し、エージェント・ペインの上にボタンを置き、コマンドパレットに項目を追加します。[Marketplace](https://github.com/empty-user77/Agentty-Marketplace) で探すか自分で作ってください。

[ドキュメント →](https://www.agentty.run/docs/plugins-overview)

</td>
</tr>
</table>

**その他の機能：**

- **[セッション](https://www.agentty.run/docs/sessions)** — あなたのコンピューターに保存されている Claude Code と Codex のセッションを閲覧、検索、再開できます。
- **[モニタリング](https://www.agentty.run/docs/monitoring)** — ローカル・トランスクリプト、実行中の AI プロセス、コンピューター上のすべての git ワークツリー、ビルド出力をクリアするディスク・ページからコスト、トークン、キャッシュ・ヒット率を表示。
- **[拡張、MCP、コネクター](https://www.agentty.run/docs/mcp-and-connectors)** — Claude Code と Codex 用のスキル、サブエージェント、コマンド、MCP サーバーを一か所に集約。秘密情報は OS の資格情報ストアで保管。
- **[私のアイデアをビルド](https://www.agentty.run/docs/build-my-idea)** と **[公開](https://www.agentty.run/docs/launch)** — 数文のアイデアをエージェントがビルドできるプロジェクトに変え、GitHub、Vercel、Supabase で公開。
- **[Claude アドバイザー](https://www.agentty.run/docs/advisor)** — Claude Code が必要な瞬間に更に強力なモデルに相談させる — タブごとに設定可能。
- **ファイルとエディタ** — プロジェクト・ツリーと構文色付けとフォーマッターを備えたエディタ、加えて「VS Code または Cursor で開く」。
- **コマンドパレット、未読へジャンプ、ミニモード** — ⇧⌘P、⇧⌘U、⌃⌘M。[ショートカット](#ショートカット)を参照。

---

## 対応エージェント

**あらゆる CLI エージェント** に対応 — ターミナルで実行できるなら、Agentty で実行できます。Agentty はこれらを検知してランチャーに表示します：

<p>
  <a href="https://docs.anthropic.com/en/docs/claude-code"><kbd><img src="https://www.google.com/s2/favicons?domain=claude.ai&amp;sz=64" alt="" width="16" valign="middle" /> Claude Code</kbd></a> &nbsp;
  <a href="https://github.com/openai/codex"><kbd><img src="https://www.google.com/s2/favicons?domain=openai.com&amp;sz=64" alt="" width="16" valign="middle" /> Codex</kbd></a> &nbsp;
  <a href="https://github.com/google-gemini/gemini-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=gemini.google.com&amp;sz=64" alt="" width="16" valign="middle" /> Gemini CLI</kbd></a> &nbsp;
  <a href="https://antigravity.google"><kbd><img src="https://www.google.com/s2/favicons?domain=antigravity.google&amp;sz=64" alt="" width="16" valign="middle" /> Antigravity CLI</kbd></a> &nbsp;
  <a href="https://ampcode.com"><kbd><img src="https://www.google.com/s2/favicons?domain=ampcode.com&amp;sz=64" alt="" width="16" valign="middle" /> Amp</kbd></a> &nbsp;
  <a href="https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=github.com&amp;sz=64" alt="" width="16" valign="middle" /> GitHub Copilot CLI</kbd></a> &nbsp;
  <a href="https://cursor.com/cli"><kbd><img src="https://www.google.com/s2/favicons?domain=cursor.com&amp;sz=64" alt="" width="16" valign="middle" /> Cursor CLI</kbd></a> &nbsp;
  <a href="https://opencode.ai"><kbd><img src="https://www.google.com/s2/favicons?domain=opencode.ai&amp;sz=64" alt="" width="16" valign="middle" /> OpenCode</kbd></a> &nbsp;
  <a href="https://github.com/QwenLM/qwen-code"><kbd><img src="https://www.google.com/s2/favicons?domain=qwenlm.github.io&amp;sz=64" alt="" width="16" valign="middle" /> Qwen Code</kbd></a> &nbsp;
  <a href="https://docs.factory.ai/cli/getting-started/quickstart"><kbd><img src="https://www.google.com/s2/favicons?domain=factory.ai&amp;sz=64" alt="" width="16" valign="middle" /> Factory Droid</kbd></a> &nbsp;
  <a href="https://block.github.io/goose/"><kbd><img src="https://www.google.com/s2/favicons?domain=goose-docs.ai&amp;sz=64" alt="" width="16" valign="middle" /> Goose</kbd></a> &nbsp;
  <a href="https://github.com/charmbracelet/crush"><kbd><img src="https://www.google.com/s2/favicons?domain=charm.sh&amp;sz=64" alt="" width="16" valign="middle" /> Crush</kbd></a> &nbsp;
  <a href="https://aider.chat"><kbd><img src="https://aider.chat/assets/icons/favicon-32x32.png" alt="" width="16" valign="middle" /> Aider</kbd></a> &nbsp;
  <a href="https://github.com/MoonshotAI/kimi-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=moonshot.cn&amp;sz=64" alt="" width="16" valign="middle" /> Kimi CLI</kbd></a> &nbsp;
  <a href="https://kiro.dev/docs/cli/"><kbd><img src="https://www.google.com/s2/favicons?domain=kiro.dev&amp;sz=64" alt="" width="16" valign="middle" /> Kiro CLI</kbd></a> &nbsp;
  <a href="https://docs.cline.bot/cline-cli/overview"><kbd><img src="https://www.google.com/s2/favicons?domain=cline.bot&amp;sz=64" alt="" width="16" valign="middle" /> Cline CLI</kbd></a> &nbsp;
  <a href="https://x.ai/cli"><kbd><img src="https://www.google.com/s2/favicons?domain=x.ai&amp;sz=64" alt="" width="16" valign="middle" /> Grok Build</kbd></a> &nbsp;
  <a href="https://ollama.com"><kbd><img src="https://www.google.com/s2/favicons?domain=ollama.com&amp;sz=64" alt="" width="16" valign="middle" /> Ollama (ローカルモデル)</kbd></a> &nbsp;
  <kbd>+ あらゆる CLI エージェント</kbd>
</p>

Claude Code と Codex には第一級のペインがあります： フックからのライブ状態、セッション、使用量とコスト、二つ間の引き継ぎ。

---

## インストール

### macOS — Apple シリコン、macOS 13 以降

```sh
brew install --cask empty-user77/agentty/agentty
```

これで Agentty がアプリケーション・フォルダに入ります。Homebrew はまだですか？[brew.sh](https://brew.sh) の 1 行のコマンドで先にインストールしてから。Agentty は自動更新；`brew upgrade --cask agentty` も使えますし、`brew uninstall --cask agentty` で削除できます。

直接ダウンロード希望ですか？[最新リリース](https://github.com/empty-user77/Agentty/releases/latest)から `Agentty-X.Y.Z-…-arm64.dmg` を取得して開き、**Agentty** を **アプリケーション** にドラッグします。Apple に署名・公証されています。Intel Mac 用のビルドはありません。

### Windows — 10 バージョン 1809 以降、x64

1. [最新リリース](https://github.com/empty-user77/Agentty/releases/latest)から `Agentty-X.Y.Z-windows-x64-setup.exe` をダウンロード。ブラウザが `.exe` をブロックしたら、`Agentty-X.Y.Z-windows-x64-setup.zip` を取得して展開 — 同じインストーラーが入っています。
2. 実行します。ユーザー単位でのインストールで、管理者権限は不要です。
3. インストーラーはまだコード署名されていません。SmartScreen が現れたら **詳細情報 → 実行** を選んでください。

アンインストール： 設定 → アプリ → Agentty → アンインストール。

### Linux — x86_64

[最新リリース](https://github.com/empty-user77/Agentty/releases/latest)からあなたのディストリビューション用パッケージをダウンロード、その後：

```sh
# Debian 12+ / Ubuntu 22.04+
sudo apt install ./Agentty-*-linux-amd64.deb

# RHEL 9+ / Fedora
sudo dnf install ./Agentty-*-linux-x86_64.rpm
```

アンインストール： `sudo apt remove agentty` または `sudo dnf remove agentty`。

### アップデート

Agentty は起動時と 1 時間ごとに新しいバージョンを確認します。macOS と Windows ではアップデートをダウンロード、リリース・チェックサムで検証、インストール、再起動します。Linux では新しいバージョンがあることを知らせ、パッケージ・ページへリンクします。

### 始める前に

Agentty はエージェントを実行します。バンドルしていません。使いたい CLI — 例えば [Claude Code](https://docs.anthropic.com/en/docs/claude-code)（`claude`）と [Codex](https://github.com/openai/codex)（`codex`）— をインストールして、`PATH` にあることを確認してください。Agentty の 設定 → システム で足りないものをインストールすることもできます。

### ソースからビルド

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

Rust 1.98（`rust-toolchain.toml` で固定）とプラットフォーム別ビルド・ツールが必要。Windows と Linux は [docs/platforms.md](docs/platforms.md) を参照。

---

## ショートカット

| 操作 | ショートカット |
|---|---|
| 新しいターミナル / Claude Code / Codex タブ | ⌘T / ⌥⌘C / ⌥⌘X |
| 右に分割 / 下に分割 | ⌘D / ⇧⌘D |
| 次・前のペイン · タブ · ワークスペース | ⌘] ⌘[ · ⇧⌘] ⇧⌘[ · ⌥⌘↓ ⌥⌘↑ |
| コマンドパレット · 未読へジャンプ | ⇧⌘P · ⇧⌘U |
| Git · Session Flow · モニタリング · 設定 | ⇧⌘G · ⇧⌘F · ⌥⌘U · ⌘, |
| ファイルパネル · アプリ内ブラウザ · ミニモード | ⌥⌘B · ⇧⌘B · ⌃⌘M |

すべての一覧は **設定 → キーボード・ショートカット** にあります。Windows と Linux では ⌘ が Ctrl+Shift、⇧⌘ が Ctrl+Alt+Shift、⌥⌘ が Ctrl+Alt になり、Ctrl+文字はシェルに渡ります。

## プラグイン

プラグインはターミナルの横のパネルを埋め、エージェント・ペインの上にボタンを置き、コマンドパレットに項目を追加し、プロンプトを送れます — 送り先は必ず「送信先を選択」ダイアログであなたが選びます。プラグインのパーミッションはインストール前にプラグイン・ページで確認でき、Agentty に何ができるかを限定します。WebAssembly プラグインはそのパーミッションの外には届きません。Node.js、Python、またはネイティブ・プログラムとして動くプラグインは、Agentty の外ではあなたと同じファイルとネットワークへのアクセス権を持つため、信頼できるもののみインストール。プラグインは [Agentty Marketplace](https://github.com/empty-user77/Agentty-Marketplace) で配布。

組み込みの **Cosmica** プラグインは [Cosmica](https://www.cosmica.ink/) ノートをプロンプトに変え、セッション要約を書き戻します。自分で作るには [docs/plugins](docs/plugins/README.md) を参照。プラグイン・ページから新規作成して Claude Code に実装させることもできます。

## ドキュメント

| | |
|---|---|
| [agentty.run/docs](https://www.agentty.run/docs) | アプリの各部分がすることの説明 |
| [docs/ja](docs/ja/introduction.md) | ユーザー・ドキュメント（[English](docs/en/introduction.md) · [한국어](docs/ko/introduction.md) · [中文](docs/zh/introduction.md) と同じ） |
| [docs/configuration.md](docs/configuration.md) | `settings.json`、エージェント・サインイン、ハーネス、データベース、テーマ、Agentty が作るファイル |
| [docs/platforms.md](docs/platforms.md) | Windows と Linux での違いと、そこでのビルド方法 |
| [docs/plugins](docs/plugins/README.md) | プラグイン作成（Node.js SDK または [プロトコル](docs/plugins/protocol.md)）と[使用方法](docs/plugins/usage.md) |
| [docs/architecture.md](docs/architecture.md) | クレート、ターミナル、エージェント・ソケット、パネルがどう組み立てられているか |
| [docs/metrics.md](docs/metrics.md) | 公式ビルドが送信できるすべてのイベントと、オフにする方法 |
| [docs/release.md](docs/release.md) | パッケージング、署名、公証とリリース・チャネル |

---

## コミュニティと支援

- **X:** [@agentty_run](https://x.com/agentty_run) をフォロー — アップデートとお知らせ。
- **フィードバック・アイデア:** 何か足りませんか？バグを見つけました？[issue を開く](https://github.com/empty-user77/Agentty/issues)。
- **セキュリティ:** 脆弱性は [SECURITY.md](SECURITY.md) の説明に従って非公開で報告。
- **貢献:** [CONTRIBUTING.md](CONTRIBUTING.md) と [Code of Conduct](CODE_OF_CONDUCT.md) をお読みください。
- **プライバシー:** エージェント・トランスクリプトは **ローカルでのみ** 読まれます — セッション表示、使用量計算、ハンドオフ構築のため。プロンプト、出力、パス、リポジトリ・データが送信されることはありません。公式リリース・ビルドは匿名使用統計（使った機能、アプリとシステム・バージョン、ランダムなインストール ID）を送ります。設定 → 一般 でオフ、または `DO_NOT_TRACK=1` でオフ。ソースからビルドしたものは何も送りません。すべてのイベントは [docs/metrics.md](docs/metrics.md) に表示。
- **サポート:** このリポジトリに [star](https://github.com/empty-user77/Agentty) を付けて追跡。

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

全文は [LICENSE](LICENSE)。同梱フォントとサード・パーティ構成要素はそれぞれのライセンス — [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) を参照。
