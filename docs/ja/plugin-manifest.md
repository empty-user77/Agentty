---
title: マニフェスト リファレンス
description: agentty-plugin.json のすべてのフィールド — 識別情報、実行方法、権限、パネル、コマンド、アイコン。
---

`agentty-plugin.json` はプラグインフォルダの直下に置かれ、そのプラグインが何であり、どう起動し、何ができ、インターフェースに何を加えるかを Agentty に伝えます。

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "runtime": "node",
  "apiVersion": 1,
  "description": "Turns the current folder into a prompt.",
  "publisher": "you",
  "homepage": "https://example.com/hello",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [
      { "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }
    ]
  }
}
```

## 識別情報

| フィールド | 必須 | 説明 |
|---|---|---|
| `id` | はい | `a-z`、`0-9`、`-` からなる 2〜40 文字。**フォルダ名と一致させてください。** |
| `name` | はい | ストアとパネルボタンに表示 |
| `version` | はい | `major.minor.patch` |
| `description` | | ストアカードの 1 行説明 |
| `publisher` | | 作者 |
| `homepage` | | `https://` である必要があります |
| `keywords` | | ストア検索用 |
| `links` | | カードにボタンとして出る `{ "label", "url" }` を最大 6 件（プロジェクトサイト、ドキュメント、ソース） |
| `icon` | | 下記一覧のアイコン名 |
| `logo` | | プラグインフォルダ内の画像ファイル。`icon` の代わりに描画されます。モジュールはロゴをモジュール内に持ちます — [ロゴ](#ロゴ)を参照 |

## 実行

| フィールド | 既定値 | 説明 |
|---|---|---|
| `main` | 必須 | プラグインフォルダからの相対パスのエントリポイント |
| `runtime` | `node` | `node`（ログインシェル `PATH` の Node.js 18+）、`python`（`python3 <main>`）、`executable`（`<main>` を直接実行）、または `wasm` — `<main>` が Agentty 自身の実行する WebAssembly モジュール。[Rust と WebAssembly](/docs/plugin-rust) を参照 |
| `apiVersion` | `1` | 作成時に基準としたプラグイン API のバージョン。`2` は [AgentOS プラグイン](/docs/plugin-agentos)に必要な `host/timer` と `pane/status` を加えます。古いバージョンしか話せない Agentty は、実行できないものを入れる代わりにその旨を伝えます |
| `activationEvents` | `[]` | `["onStartup"]` なら Agentty と同時に起動、なければ最初の使用時 |

Agentty はプラグインフォルダを作業ディレクトリとしてプログラムを起動します。`wasm` プラグインはプログラムを起動しません。モジュールは Agentty の中で動き、作業ディレクトリも環境変数もファイルもありません。

## 他のアプリとの連携

| フィールド | 説明 |
|---|---|
| `requires` | `{ "name", "url", "note" }` — このプラグインが対象とするアプリやサービス。カードが検出の有無を示し、なければリンクを出します |
| `detect` | 連携先アプリのパス（`~` 可）。見つかればカードに**おすすめ**が付きます |

## 権限

```json
"permissions": ["net.request", "prompt.inject", "terminal.write", "session.read", "workspace.read"]
```

| 権限 | できること |
|---|---|
| `net.request` | プラグインが指定したアドレスへの HTTP リクエスト |
| `prompt.inject` | プロンプトの送信 |
| `terminal.write` | 開いているペインへの入力 |
| `session.read` | AI 会話の読み取り |
| `workspace.read` | ワークスペース一覧の取得、コンテキストのフォルダ・タイトル項目の参照 |

使うものだけを求めてください。一覧はインストール前に利用者へ表示され、権限のない呼び出しは失敗します。[プラグインの権限](/docs/plugin-permissions)を参照してください。

## プラグインが加えるもの

### パネル

```json
"contributes": { "panel": { "title": "Hello", "icon": "sparkles", "surface": "sidebar", "mode": "push" } }
```

プラグインにボタンと、UI ツリーで満たすパネル（幅 360 px、縦スクロール）を与えます。[SDK](/docs/plugin-sdk) を参照してください。

`surface` はボタンの位置を決めます。

| `surface` | 位置 |
|---|---|
| `pane`（既定） | ターミナル上のタブ列 |
| `sidebar` | 左端のアクティビティバー、Agentty 自身のページと並んで |
| `status` | 下部のステータスバー |

`mode` はパネルの開き方を決めます。利用者はパネルのレイアウトボタンで変更でき、その選択は保たれます。最初の動作は次のとおりです。

| `mode` | |
|---|---|
| `push`（既定） | ターミナルの横にドッキングし、ターミナルが場所を空けます |
| `overlay` | 窓の右端に浮かび、他は動きません |
| `window` | 独立した窓。移動とサイズ変更ができます |
| `full` | ターミナルとページが使う領域の全体 |

ドッキングしたパネルが残りの窓を押しつぶすほど大きくなることはありません。ドッキングできる幅を越えて引くと、浮かぶパネルになります。

### コマンド

```json
"contributes": {
  "commands": [
    {
      "id": "hello.explain",
      "title": "Hello: Explain this folder",
      "description": "Sends a tour request to the focused agent",
      "icon": "bot",
      "palette": true
    }
  ]
}
```

| フィールド | 説明 |
|---|---|
| `id` | プラグイン内で一意。SDK はこの id でハンドラを登録します |
| `title` | パレットに表示。プラグイン名を先頭に付けるとまとまって見えます |
| `description` | 任意の 2 行目 |
| `icon` | パレットでコマンドの横に表示されるアイコン名 |
| `palette` | `false` でコマンドパレットから隠す |

コマンドはコマンドパレットから実行します。`paneBar` と `when` はなくなりました。これらが残っているマニフェストもこれまで通りインストール・実行でき、フィールドは無視されます。ただし**そのボタンを前提に作られたプラグインはボタンを失い**、コマンドはパレットから実行することになります。

## アイコン

`icon` には次の名前を使います。それ以外はパズルのピースとして表示されます。

```
app-window arrow-down arrow-left arrow-right arrow-up arrow-up-right at-sign bell bell-dot
blocks book-open bookmark bot brain bug calendar chart-column check chevron-down chevron-right
chevron-up circle-check circle-dot circle-pause circle-x clipboard clipboard-paste clock cloud
code columns-2 command container copy database download ellipsis external-link eye file-input
file-plus file-text folder folder-open folder-plus git-branch git-commit-horizontal
git-pull-request globe grip-vertical hammer hash history house image info key-round
layout-panel-left lightbulb link list list-tree loader-circle mail maximize-2 message-circle-question
message-square minimize-2 minus network notebook notebook-pen package panel-left-close
panel-left-open pencil picture-in-picture-2 play plug plus power puzzle refresh-cw rocket rotate-cw
rows-2 save scroll-text search send settings shield-alert sparkles square square-plus
square-terminal star sticky-note tag terminal trash-2 undo-2 unlink upload users wand-sparkles
workflow wrench x zap git-fork file lock graduation-cap x-twitter
```

## ロゴ

ロゴはプラグイン自身の画像です。ストアの一覧、詳細カード、パネルのボタンで、`icon` の名前の代わりにこの画像が描画されます。

**ロゴはどこからも取得しません。**プラグインと一緒に運ばれるので、画面に描かれる絵はユーザーがインストールしたその絵です。そしてロゴを付けても、誰がインストールしたかが作者に伝わることはありません。

| プラグインの種類 | ロゴのある場所 |
|---|---|
| フォルダ — `node`、`python`、`executable`、または開発用にリンクしたフォルダ | `"logo": "logo.png"`、プラグインフォルダ内のファイル |
| 単一の `wasm` モジュール — ストアから来るすべて | モジュールが画像を自分で持ちます。`logo` は使いません。[Rust と WebAssembly](/docs/plugin-rust#ロゴ)を参照 |

マニフェストのフィールドとしての `logo` はファイル名でしかありません。プラグインフォルダの外に出るパス（`../`）、絶対パス、スキームが付いた値（`https://` のアドレスを含む）、400 文字を超える値はいずれも拒否され、プラグインは `icon` をそのまま使います。

モジュールが持つ画像は **PNG・JPEG・GIF・WebP のいずれか**で、名前ではなく**バイトで実際の形式を確認**し、512KB 以下である必要があります。それ以外はロゴではなく、プラグインは `icon` をそのまま使います。

**モジュールのロゴが SVG であることはありません。**どう名乗っていても同じです。SVG は絵ではなく文書だからです — レンダラーがその中に書かれたアドレスを解決するため、ローカルのパスが書かれていればそのファイルが開かれて描画されます。プラグインがロゴと称してユーザー自身のファイルを画面に出せてはなりません。

## 環境変数

プログラムとして動くプラグイン（`node`, `python`, `executable`）には次の環境変数が渡されます。`wasm` プラグインには何も渡されず、必要なものは[自身のストレージ](/docs/plugin-permissions)に置きます。

| 変数 | 意味 |
|---|---|
| `AGENTTY_PLUGIN_ID` | プラグインの id |
| `AGENTTY_PLUGIN_DIR` | プラグインフォルダ |
| `AGENTTY_PLUGIN_DATA` | 設定とキャッシュのための専用フォルダ |
| `AGENTTY_VERSION` | Agentty のバージョン |
| `AGENTTY_LANGUAGE` | 利用者の言語（`en`、`ko`、`ja`、`zh`） |
| `AGENTTY_BIN` | `agentty` コマンドラインツールのパス |

保存するものはすべて `AGENTTY_PLUGIN_DATA` の中か、どちらの種類でも動く `storage/*` に置いてください。
