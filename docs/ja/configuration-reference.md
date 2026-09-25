---
title: 設定リファレンス
description: Agentty が保持するすべてのファイルと、settings.json のすべてのキー。
---

Agentty はすべてを `~/.agentty/` に保存します。ほとんどは**設定**（⌘,）から変えるほうが簡単です。

## ファイル

| パス | 用途 |
|---|---|
| `~/.agentty/settings.json` | 設定 |
| `~/.agentty/workspaces.json` | ワークスペース、タブ、分割、グループ |
| `~/.agentty/themes/*.itermcolors` | 読み込んだカラーテーマ |
| `~/.agentty/pricing.json` | Claude 以外のモデルの価格 |
| `~/.agentty/handoffs/` | Session Flow とエージェント移行が作るコンテキスト文書 |
| `~/.agentty/connectors.json` | API コネクタの定義（秘密の値なし） |
| `~/.agentty/agent-auth.json` | 新しいエージェントタブのサインイン方法（秘密の値なし） |
| `~/.agentty/codex-home/` | API キーでサインインするための専用 Codex ホーム |
| `~/.agentty/worktrees/` | セッションごとに作られたワークツリー |
| `~/.agentty/plugins/` | 導入済みプラグイン |
| `~/.agentty/plugin-data/<id>/` | 各プラグインのデータ |
| `~/.agentty/prompts/` | ファイルとして渡される長いプロンプト |
| `~/.agentty/install_id` | 使用統計のためのランダムなインストール識別子 |
| `agentty.json`（プロジェクト）または `~/.agentty/commands.json` | コマンドパレットの独自項目 |

秘密の値はこれらのファイルに入りません。OS の資格情報ストアにあります。

## settings.json

```json
{
  "language": "ja",
  "theme": "Agentty Dark",
  "fontFamily": "JetBrains Mono",
  "fontSize": 13.0,
  "lineHeight": 1.25,
  "cursorShape": "block",
  "scrollback": 10000,
  "askDirectory": true,
  "autoWorktree": true,
  "agentTasks": true,
  "analytics": true,
  "agentBarPosition": "top"
}
```

| キー | 値 |
|---|---|
| `language` | `en`、`ko`、`ja`、`zh` |
| `theme` | 内蔵テーマ名、または読み込んだ `.itermcolors` の拡張子なしのファイル名 |
| `fontFamily`、`fontSize`、`lineHeight`、`padding` | 外観 |
| `cursorShape` | `block`、`beam`、`underline`。`cursorBlink` で点滅 |
| `letterSpacing` | ターミナルのセルごとに加える幅（pt、0–8） |
| `boldText` | 通常のテキストを太字で描画。元から太字の部分はさらに太くなる |
| `colorBackground`、`colorForeground`、`colorCursor`、`colorSelection` | 手で置き換えたテーマの色。`0xRRGGBB` を数値で書く — JSON に 16 進リテラルはないので `0x121212` は `1184274` |
| `scrollback` | ターミナルごとに保持する行数 |
| `askDirectory` | 新しいワークスペースでフォルダを尋ねる。`askDirectoryForTabs` はタブに対して同様 |
| `resumeBar` | 以前のセッションがあるフォルダに入ったら再開を提案 |
| `autoWorktree` | 作業中のプロジェクトの 2 つ目のセッションに専用ワークツリー |
| `agentTasks` | エージェントが並列タスクの開始を要求できるようにし、毎回確認 |
| `agentGuide` | ここで起動したエージェントに Agentty のコマンド案内を渡す |
| `stopServersOnClose` | ペインを閉じたら、そこで起動したサーバーを終了 |
| `confirmClose` | 使用したものを閉じる前に確認 |
| `agentBarPosition` | `top` または `bottom` |
| `workspaceSearchBar` | ワークスペース一覧の上の検索欄。名前と、その中の会話を検索（既定でオン） |
| `sortFinishedToTop` | エージェントが終えたワークスペースを一番上へ（既定でオフ。オフなら自分で並べた順序のまま） |
| `hud` | ステータスバーの項目と順序: `model`、`context`、`usage`、`status`、`elapsed`、`links`、`spacer`、`ports`、`worktree`、`branch`、`folder` |
| `browser.autoOpenServers` | 開発サーバーが応答したらアプリ内ブラウザで開く |
| `linkOpener` | ⌘-クリックがアプリ内ブラウザか既定のブラウザか |
| `externalEditor` | `auto`、`vsCode`、`cursor`、`system` |
| `harnessDetect`、`harnessPatterns`、`harnessSubmit`、`harnessAgent` | ハーネスの検出と挙動 |
| `systemNotifications`、`notifyWhenFocused`、`notifyAnswerRequests`、`chatNotify` | 通知 |
| `chatNotify.slackBot`、`chatNotify.discordBot` | Webhook URL ではなくボットトークンとチャンネルで送る |
| `chatNotify.slackChannel`、`chatNotify.discordChannel` | ボットが書き込むチャンネル。Slack は `#general`・`general`・チャンネル ID、Discord はチャンネル ID |
| `analytics` | 匿名の利用統計への同意。`false` または `DO_NOT_TRACK=1` なら何も送らない |
| `menuBar` | メニューバーのアイコン（macOS） |

## 独自コマンド

プロジェクトの `agentty.json`、またはすべてのプロジェクトに適用される `~/.agentty/commands.json` でコマンドパレットに項目を追加します。

```json
{
  "commands": [
    { "label": "Run tests", "command": "npm test" },
    { "label": "Deploy staging", "command": "./scripts/deploy.sh staging" }
  ]
}
```

## pricing.json

価格が内蔵されていないモデル向けの、100 万トークンあたりの USD です。

```json
{
  "some-model": { "input": 1.25, "output": 10.0, "cacheRead": 0.125 }
}
```

`cacheRead`、`cacheWrite`、`cacheWrite1h` は任意で、既定は `input` の 10%、125%、200% です。
