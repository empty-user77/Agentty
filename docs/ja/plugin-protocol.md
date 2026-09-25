---
title: プラグイン プロトコル
description: SDK の背後の JSON-RPC ワイヤーフォーマット、WebAssembly モジュールの ABI、そしてすべてのメッセージ・制限・エラーコード。
---

SDK なしでプラグインを書くためのワイヤーフォーマットです。[Node.js SDK](/docs/plugin-sdk) と [Rust SDK](/docs/plugin-rust) がすべて包んでくれます。どちらにせよ[クイックスタート](/docs/plugin-quickstart)を先に読むとよいでしょう。

API **バージョン 1** は、このページから `host/timer` と `pane/status` を除いたすべてです。この 2 つは**バージョン 2** です。

## 通信方式

Agentty はプラグインフォルダを作業ディレクトリとしてプログラムを起動します。

| `runtime` | 実行コマンド |
|---|---|
| `node` | `node <main>` — ログインシェルの `PATH`、Homebrew、Volta、nvm の Node.js |
| `python` | `python3 <main>` |
| `executable` | `<main>` |
| `wasm` | なし — `<main>` が Agentty 自身の実行する WebAssembly モジュール。[WebAssembly プラグイン](#webassembly)を参照 |

メッセージは [JSON-RPC 2.0](https://www.jsonrpc.org/specification) のオブジェクトで、**1 行に 1 つ**、UTF-8 で、stdin（Agentty → プラグイン）と stdout（プラグイン → Agentty）をやり取りします。16MB を超える行は拒否されます。stdout の JSON でない内容はログに残って無視され、stderr はプラグインログへ送られます。

stdin が閉じるか `shutdown` が来たら終了してください。`shutdown` から 1.5 秒経っても生きていると `SIGTERM`、さらに 1.5 秒後に `SIGKILL` が送られます。Agentty の終了時は両方が即座に続きます。

## Agentty → プラグイン

| メッセージ | 種類 | `params` |
|---|---|---|
| `initialize` | リクエスト — 応答が必要 | `{ apiVersion, agentty: { version }, plugin: { id, name, version, dir, dataDir }, language, context }` |
| `command/execute` | 通知 | `{ command, args, context }` |
| `panel/open` · `panel/close` | 通知 | `{ context }` |
| `ui/event` | 通知 | `{ element, event, value?, item?, action?, context }` |
| `context/changed` | 通知 | `{ context }` |
| `url/open` | 通知 | `{ path, query, url, context }` |
| `pane/status` | 通知 | `{ paneId, status, running, agent, title, cwd }` — このプラグインが開始したペインの状態が変わった（`workspace.read`、API 2） |
| `shutdown` | 通知 | `{}` |

`initialize` が最初に来て、その直後にプラグインを起動させたもの（コマンド、パネルを開く、リンク）が続きます。`initialize` には任意の結果で応答すれば十分です（例: `{}`）。

## プラグイン → Agentty

結果やエラーが必要ならリクエスト（`id` あり）として、不要なら通知（`id` なし）として送ります。

| メソッド | 権限 | `params` | 結果 |
|---|---|---|---|
| `ui/setPanel` | | `{ tree }` | `null` |
| `ui/showPanel` | | `{}` | `null` |
| `ui/notify` | | `{ message, kind }` — `info`、`success`、`warning`、`error` | `null` |
| `ui/setBadge` | | `{ text }`、最大 8 文字 | `null` |
| `context/get` | | `{}` | コンテキスト |
| `host/info` | | `{}` | `{ version, apiVersion, language }` |
| `host/openUrl` | | `{ url }` — http/https | `null` |
| `host/copy` | | `{ text }` — 最大 100,000 文字 | `null` |
| `host/timer` | | `{ ms }` — API 2 | 時間が経つと `{ elapsedMs }` |
| `host/revealPath` | `workspace.read` | `{ path }` — 存在する絶対パス | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, cwd?, submit? }` | `{ status: "asked" }` または `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` — `paneId` がなければフォーカス中のペイン | `{ paneId }` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` — 既定 200、最大 2000 | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes }]` |
| `net/fetch` | `net.request` | `{ url, method?, headers?, body?, timeoutMs?, proxy? }` | `{ status, statusText, url, headers, body, truncated, binary, bytes, durationMs }` |
| `storage/get` | | `{ key }` | `{ key, value }` — 未設定なら `value` は null |
| `storage/set` | | `{ key, value }` — null で削除 | `null` |
| `storage/keys` | | `{}` | `[key]` |

700ms より速く届く `ui/notify` は正常に応答したうえで破棄され、毎秒 240 件を超えて送るプラグインは停止されます。コンテキストの項目はプラグインの権限で制限されます。

### ネットワークに届く

`net/fetch` はプラグインがネットワークに届く唯一の方法です。Agentty が課す範囲 — メソッド、ヘッダ、サイズ、タイムアウト、リダイレクト — は[権限](/docs/plugin-permissions#net-request)にあります。要点は、リクエストにあなたのものは何も載らないということです。クッキーも保存された資格情報もなく、プラグインが自分で入れたものだけが行きます。

### 待つ、そしてエージェントが終わったのを聞く

`host/timer` はプラグインが待つ方法です。時間が経つと応答されるリクエストで、最短 100 ms、最長 1 時間、同時に 8 件まで。モジュールはメッセージを処理している間しか動かないので、あとの時点へ戻ってくる手段はこれがすべてです。得られるのはその応答だけなので、バックグラウンド実行の手段にはなりません。

`pane/status` は、プラグインが開始したエージェントが終わったのを聞く方法です。プラグインは `prompt/inject` の応答（`{ status: "sent", paneId }`）でペイン id を知り、Agentty はどのプラグインがどのペインを開始したかを覚えていて、そのプラグインにだけ状態変化を伝えます。`working`、`idle`、`finished`、`permission`、`question`、`interrupted`、`exited`、そして最後に一度 `closed` です。同時に最大 32 ペインまで追跡します。

利用者が自分で置いたプロンプトも追われます。`target: "ask"` はまだペインがないのでペイン id なしに `{ status: "asked" }` を返しますが、利用者が選んだセッションも同じように見守られ、その最初の `pane/status` が、どのペインになったかをプラグインが知る場所です。未解決の問いが複数あるプラグインは、プロンプトに付けた `title` で見分けます。

描画されているかどうかに関係なく状態は届きます。他の窓の背後にある窓やロック画面の窓は描画されませんが、エージェントを待っているプラグインが利用者の戻りを待っていてはいけないからです。[AgentOS プラグイン](/docs/plugin-agentos)はこの 2 つの上に作られています。

### 覚えておく

`storage/*` はプラグインが実行のあいだに覚えておく手段です。自分のフォルダにある JSON ドキュメント 1 つ（`<データフォルダ>/plugin-data/<plugin>/storage.json`、`0600` で作成）をキーで読み書きします。キーは小文字・数字・`.`・`-`・`_` で、最大 64 個、合計 1 MB です。プロセスとして動くプラグインは自分のファイルを書けますが、WebAssembly プラグインにはファイルがないので、何かを残す手段はこれだけです。

### エラーコード

| コード | 意味 |
|---|---|
| `-32601` | 未知のメソッド |
| `-32602` | 不正なパラメータ — UI ツリーの誤り、存在しないペインなど |
| `-32001` | 権限がない、またはリンクが届いたための制限中 — 再起動するまで |
| `-32002` | 利用できない — ウィンドウが開いていない、セッションがまだない |

## やり取りの例

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":1,"plugin":{"id":"hello"},"language":"en","context":{}}}
→ {"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}
← {"jsonrpc":"2.0","id":1,"result":{}}
← {"jsonrpc":"2.0","id":1,"method":"ui/setPanel","params":{"tree":{"type":"column","children":[{"type":"button","id":"go","label":"Go"}]}}}
→ {"jsonrpc":"2.0","id":1,"result":null}
→ {"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"click","context":{}}}
← {"jsonrpc":"2.0","id":2,"method":"prompt/inject","params":{"text":"Hello","target":"ask"}}
→ {"jsonrpc":"2.0","id":2,"result":{"status":"asked"}}
```

`→` は Agentty からプラグイン、`←` はプラグインから Agentty です。リクエスト id は方向ごとに数えます。

## WebAssembly プラグイン

`"runtime": "wasm"` を指定すると、`main` は Agentty が自分の中でインタプリタとして実行する `.wasm` モジュールになります。1 つのファイルが macOS・Windows・Linux でそのまま動きます。

モジュールは Agentty が渡した関数だけを呼べます。ファイルもソケットも環境変数もプロセスもなく、カウンタ以外に時計もありません。WebAssembly プラグインはどう書かれていても `~/.agentty` も、利用者のプロジェクトも、資格情報も読めません。Agentty に求めるものは、プロセス型プラグインが stdout に書くのと同じメッセージで要求し、マニフェストの権限も同じように検査されます。

モジュールが export するもの:

| Export | 意味 |
|---|---|
| `memory` | 線形メモリ — Rust や C のモジュールの標準的な export |
| `agentty_alloc(len: i32) -> i32` | Agentty がメッセージを書き込む `len` バイトのバッファ |
| `agentty_on_message(ptr: i32, len: i32)` | Agentty からの UTF-8 JSON メッセージ 1 件 |

そして `agentty` という名のモジュールから import するもの:

| Import | 意味 |
|---|---|
| `send(ptr: i32, len: i32)` | Agentty へ送る UTF-8 JSON メッセージ 1 件 |
| `log(ptr: i32, len: i32)` | プラグインのログに残す 1 行 |
| `now_ms() -> i64` | Unix エポックからのミリ秒 |

メッセージは stdio と同じ JSON-RPC オブジェクトで、呼び出しごとに 1 件ずつ、改行なしで渡されます。**これ以外を import するモジュールは読み込まれません。** 64 MB を超えるモジュールも拒否され、メモリは 64 MB に制限され、メッセージごとに作業量の予算があります。戻ってこないプラグインは「時間内に終わらなかった」として停止され、1 件を処理する間に 256 件を超えるメッセージを送るプラグインも停止されます。`send` と `log` は合算されるので、ログだけのループも無料ではありません。

[Rust SDK](/docs/plugin-rust) がこれらをすべて隠してくれます。

## UI ツリー

すべてのノードは `type` を持つオブジェクトです。

```
column   { children, gap? }                gap: none | small | medium | large
row      { children, gap?, wrap? }
section  { title, children }
text     { text, style? }                  style: body | title | muted | small | code | error | success
button   { id, label, icon?, variant?, disabled? }   variant: primary | secondary | ghost | danger
input    { id, placeholder?, value?, rows? }
         rows > 1: その行数のテキストエリア（最大 24）。Enter は改行し、
         貼り付けは改行を保ちます
list     { id, items, empty? }
         items: [{ id, title, subtitle?, detail?, icon?, tone?, actions?: [{ id, label?, icon?, tooltip? }] }]
choice   { id, options: [{ value, label }], value? }
toggle   { id, label, value? }
badge    { text, tone? }                   tone: neutral | info | success | warning | error
spinner  { text? }
divider  {}
```

イベント: `button` は `click`、`input` は `value` を伴う `change`・`submit`、`list` は `item` を伴う `select`（行のボタンは `item`・`action` を伴う `action`）、`choice` は選んだ値を伴う `change`、`toggle` は新しい真偽値を伴う `change` を送ります。

リスト項目の `tone` はアイコンの色を決め、`badge` と同じ値を使います。

## Agentty なしでテストする

`node`・`python`・`executable` のプラグインは stdin を読み stdout に書く普通のプログラムなので、テストから直接動かせます。`initialize` リクエストを書き、試したい通知を続けて送り、プラグインが返す JSON を検証してください。

`wasm` モジュールも同じやり方で、上の 3 つの import を用意できる WebAssembly ランタイムであれば動かせます。
