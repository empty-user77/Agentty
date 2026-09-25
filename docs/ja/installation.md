---
title: インストール
description: macOS・Windows・Linux 向けの Agentty を入手し、一緒に使うエージェント CLI を用意する方法。
---

Agentty は無料でアカウントも不要です。ダウンロードしてドラッグし、ターミナルを開くだけです。

## ダウンロード

すべてのリリースは[リリースページ](https://github.com/empty-user77/Agentty/releases)で公開されます。

**macOS 13 以降、Apple シリコン** — `Agentty-X.Y.Z-…-arm64.dmg` を開き、**Agentty** を**アプリケーション**にドラッグします。Developer ID で署名され Apple の公証を受けているため、警告なしに起動します。

> [!NOTE]
> Intel Mac 向けのビルドはありません。macOS 版 Agentty は Apple シリコン専用です。

[Homebrew](https://brew.sh) ならコマンド 1 つでインストールできます: `brew install --cask empty-user77/agentty/agentty`。削除は `brew uninstall --cask agentty` です。

**Windows 10 1809 以降、x64** — `Agentty-X.Y.Z-windows-x64-setup.exe` を実行します。ユーザー単位でインストールされ、管理者権限は不要です。

> [!IMPORTANT]
> **Windows インストーラーはまだコード署名されていません。** そのため Windows SmartScreen が発行元不明と警告することがあります。続けるには **詳細情報 → 実行** を選んでください。ブラウザーが `.exe` のダウンロード自体を拒む場合（Chrome は署名のない実行ファイルをブロックします）、`Agentty-X.Y.Z-windows-x64-setup.zip` をダウンロードして展開してください。同じインストーラーが入っています。

**Linux、x86_64** — Debian 12+ / Ubuntu 22.04+: `sudo apt install ./Agentty-X.Y.Z-linux-amd64.deb`。RHEL 9+ / Fedora: `sudo dnf install ./Agentty-X.Y.Z-linux-x86_64.rpm`。

すべてのファイルのチェックサムは `Agentty-X.Y.Z-SHA256SUMS.txt` にあります。3 つのプラットフォームの違いは[プラットフォーム](/docs/platforms)を参照してください。

## エージェントのコマンドラインツール

Agentty はエージェントを実行しますが、同梱はしません。使いたいツールをインストールし、ログインシェルの `PATH` から見つけられるようにしてください。

- **Claude Code** — `claude`
- **Codex** — `codex`
- 任意: Gemini CLI、Copilot CLI、Cursor CLI、Grok、OpenCode、Qwen Code、Amp、Droid、Goose、Crush、Aider、ローカルの Ollama モデル

Agentty はインストール済みのものだけを検出して提示します。ないツールは後で失敗する代わりに、そもそも表示されません。

> [!TIP]
> ターミナルでは動くのに Agentty でだけ動かない場合、たいていは `PATH` を対話シェルだけが読むファイルに設定しています。Agentty はログインシェルでペインを起動するため、ログインシェルが読む場所に `PATH` を書いてください。

CLI 自体のログインを使わずに、**設定 → アカウント**から API キー、ゲートウェイトークン、`claude setup-token`、Amazon Bedrock、Google Vertex AI、あるいは取り込んだ Codex の `auth.json` で開始することもできます。

### Node.js

JavaScript で書かれたプラグインには `PATH` 上の **Node.js 18 以降**が必要です。Agentty 自体には必要ありません。

### Git

git ページ、ワークツリー、ブランチ切り替えは `PATH` の `git` を使います。Windows では Claude Code のフックに **Git for Windows** が必要です。

## アップデート

Agentty は起動時と 1 時間ごとに新しいバージョンを確認します。

通知の**後で**は、次の確認までではなく 1 日先延ばしにします。そうして待っているアップデートはステータスバーに表示され続けるので、先延ばしが見失いにはなりません。

- **macOS と Windows** — アップデートをダウンロードし、リリースのチェックサムで検証してからインストールし、再起動します。
- **Linux** — 新しいバージョンが出たことを知らせ、リリースページを案内します。新しい `.deb` または `.rpm` はパッケージマネージャーでインストールしてください。

## ソースからのビルド

ソースは [github.com/empty-user77/Agentty](https://github.com/empty-user77/Agentty) で Apache License 2.0 のもと公開されています。

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

Rust 1.98（`rust-toolchain.toml` で固定）と各プラットフォームのビルドツールが必要です。Windows と Linux で必要なものは[プラットフォーム](/docs/platforms)にあります。

## Agentty がファイルを置く場所

| パス | 用途 |
|---|---|
| `~/.agentty/settings.json` | 設定 |
| `~/.agentty/workspaces.json` | ワークスペース、タブ、分割、グループ |
| `~/.agentty/plugins/` | インストール済みプラグイン |
| `~/.agentty/handoffs/` | Session Flow と会話移行が作るコンテキスト文書 |
| `~/.agentty/connectors.json` | API コネクタの定義（秘密の値は OS の資格情報ストアに保管） |

全体の一覧は[設定リファレンス](/docs/configuration-reference)にあります。

## アンインストール

アプリケーションフォルダから `Agentty.app` を削除します。

データも消すには `~/.agentty` を削除してください。エージェント CLI が記録したセッションはそれぞれのフォルダ（`~/.claude`、`~/.codex`）にあり、Agentty が消すべきものではありません。

## 次に読むもの

- [クイックスタート](/docs/quick-start) — 最初の 10 分
- [プラットフォーム](/docs/platforms) — Windows と Linux での違い