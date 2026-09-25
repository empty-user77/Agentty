---
title: Node.js SDK
description: ハンドラ、呼び出し、パネル UI ビルダー、コンテキストオブジェクト — agentty-plugin.mjs のすべて。
---

`agentty-plugin.mjs` は依存関係のない 1 ファイルです。型定義は同じ場所の `agentty-plugin.d.ts` にあります。プラグインページで**開発者ガイド**を押すと、両方が `~/.agentty/plugins/.sdk/` に展開されます。

> [!NOTE]
> この方法で書いたプラグインは利用者のマシンでプログラムとして動き、`PATH` に Node.js 18 以降が必要で、フォルダか Git リポジトリからインストールされます。[マーケットプレイス](/docs/plugin-publishing)は WebAssembly モジュールしか受け付けません。そちらは [Rust と WebAssembly](/docs/plugin-rust) を参照してください。

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();
// ハンドラを登録…
plugin.start();
```

ハンドラはすべて `start()` の前に登録してください。

## ハンドラ

すべて async にできます。エラーはログに残り、利用者には通知として表示されます。

| ハンドラ | 呼ばれるとき |
|---|---|
| `onActivate(info => …)` | プラグインが起動した。`info` に `plugin.dataDir`、`language`、`context` |
| `command(id, ({ context, args }) => …)` | コマンドパレットからコマンドが実行された |
| `onPanelOpen(context => …)` | パネルが表示された — ここで描画します |
| `onPanelClose(context => …)` | パネルが隠れた |
| `onEvent(elementId, (event, context) => …)` | その id の UI 要素が使われた |
| `onAnyEvent((event, context) => …)` | `onEvent` が扱わなかったすべての UI イベント |
| `onContextChange(context => …)` | フォーカス中のペイン、その状態、フォルダが変わった |
| `onUrl(path, ({ path, query, url }) => …)` | `agentty://plugin/<id>/<path>?…` が開かれた |
| `onShutdown(() => …)` | Agentty がプラグインを停止させる |

## 呼び出し

すべてプロミスを返します。

| 呼び出し | 権限 |
|---|---|
| `setPanel(tree)` — パネルの内容を差し替える | |
| `showPanel()` — このプラグインのパネルを開く | |
| `notify(message, kind)` — `info`、`success`、`warning`、`error` | |
| `setBadge(text)` — タブ領域のボタンに最大 8 文字 | |
| `getContext()` | |
| `openUrl(url)` — http/https | |
| `copy(text)` — クリップボードにコピー | |
| `revealPath(path)` — ファイルマネージャーで表示 | `workspace.read` |
| `injectPrompt(request)` | `prompt.inject` |
| `sendToTerminal({ paneId, text, submit })` | `terminal.write` |
| `getSession({ paneId, maxTurns })` | `session.read` |
| `listWorkspaces()` | `workspace.read` |
| `fetch(request)` — HTTP リクエスト | `net.request` |
| `log(...)` — プラグインログ（stderr）に記録 | |

プロトコルには `storage/get`・`storage/set`・`storage/keys`（プラグイン自身のフォルダにある JSON ドキュメント、権限不要）もあり、API バージョン 2 からは `host/timer`・`pane/status` もあります。Node のプラグインは自分でファイルを書いてもかまいませんが、storage はどちらの種類でも同じように動きます。[プロトコル](/docs/plugin-protocol)を参照してください。

`plugin.info` には `initialize` のデータが、`plugin.context` には最新のコンテキストが入っています。

## パネル

パネルは幅 360px で縦にスクロールします。プラグインがツリーで記述すると Agentty がネイティブに描くため、アプリと見た目が揃いウェブビューも要りません。何かが変わるたびに新しいツリーを送ってください。テキストフィールドは別の `value` を送らない限り、利用者が入力した内容を保ちます。

| ビルダー | 要素 | イベント |
|---|---|---|
| `ui.column(children, { gap })` / `ui.row(children, { gap, wrap })` | レイアウト。`gap`: `none`、`small`、`medium`、`large` | |
| `ui.section(title, children)` | 見出し付きのまとまり | |
| `ui.text(text, style)` | `body`、`title`、`muted`、`small`、`code`、`error`、`success` | |
| `ui.button(id, label, { icon, variant, disabled })` | `primary`、`secondary`、`ghost`、`danger` | `click` |
| `ui.input(id, { placeholder, value, rows })` | 1 行入力、`rows` が 1 より大きければその行数のテキストエリア（最大 24） | 入力が止まると `change`、Enter で `submit`。`event.value` が文字列 |
| `ui.list(id, items, { empty })` | 行 `{ id, title, subtitle, detail, icon, tone, actions }` | `event.item` を伴う `select`、行のボタンは `event.item`・`event.action` を伴う `action` |
| `ui.choice(id, [{ value, label }], value)` | 分割選択 | 値を伴う `change` |
| `ui.toggle(id, label, value)` | スイッチ | 新しい真偽値を伴う `change` |
| `ui.badge(text, tone)` | `neutral`、`info`、`success`、`warning`、`error` | |
| `ui.spinner(text)` | | |
| `ui.divider()` | | |

null と false の子要素は飛ばされるので、`条件 && ui.text('…')` がそのまま動きます。

```js
plugin.onPanelOpen(async (context) => {
  const notes = await search('');
  plugin.setPanel(
    ui.column([
      ui.input('q', { placeholder: 'Search notes' }),
      ui.list('notes', notes.map((n) => ({
        id: n.path,
        title: n.title,
        subtitle: n.folder,
        icon: 'notebook',
        actions: [{ id: 'insert', icon: 'send', tooltip: 'Insert into the focused pane' }],
      })), { empty: 'No notes yet' }),
    ]),
  );
});

plugin.onEvent('notes', (event, context) => {
  if (event.event === 'action' && event.action === 'insert') {
    return plugin.injectPrompt({ text: read(event.item), target: 'ask' });
  }
});
```

> [!NOTE]
> 制限: 要素 2,000 個、深さ 12 階層、文字列 1 つあたり 20,000 文字。`choice` の選択肢とリスト項目のボタンも要素として数えます。パネルの再描画は最短 50ms 間隔、通知は最短 700ms 間隔です。毎秒 240 件を超えて送るプラグインは暴走とみなされ停止します。

## コンテキスト

すべてのコマンド・イベント・パネル呼び出しには、フォーカス中のウィンドウのコンテキストが付きます。

```json
{
  "workspace": { "id": 3, "name": "agentty", "cwd": "/Users/me/agentty", "active": true },
  "pane": {
    "id": 12,
    "kind": "claude",
    "tool": "claude",
    "title": "Claude Code",
    "cwd": "/Users/me/agentty",
    "sessionId": "…",
    "status": "idle",
    "running": true
  },
  "language": "ja"
}
```

見える範囲は宣言した権限で決まります。フォルダと名前の項目（`workspace.cwd`、`workspace.name`、`pane.cwd`、`pane.title`）には `workspace.read` が、`pane.sessionId` には `session.read` が必要です。権限がなければ id、`kind`、`tool`、`status`、`running`、言語だけが残ります。どのペインがフォーカスされているかは分かっても、利用者がどこで作業しているかは分かりません。

`kind` は `claude`、`codex`、`shell` です。他のエージェント CLI は `shell` ペインで動き、`tool` が名前を示します。

| `status` | 意味 |
|---|---|
| `idle` | 入力待ち |
| `working` | ツールを実行中 |
| `thinking` | ターンが開いており、ツール呼び出しの合間 |
| `finished` | ターンが終わった |
| `permission` | 許可を求めている |
| `question` | 利用者に質問している |
| `interrupted` | 利用者が中断した |
| `shell` | 通常のシェル |
| `exited` | プログラムが終了した |

邪魔しないかどうかを判断するときは `working` と `thinking` を同じに扱ってください。

## プロンプトを送る

```js
await plugin.injectPrompt({
  text: 'Continue the release checklist.',
  title: 'Release',          // 新規セッションのワークスペース名、ダイアログの見出し
  target: 'ask',             // ask | active | newWorkspace | newTab | pane | workspace
  agent: 'claude',           // 新規セッション用の claude | codex | shell
  cwd: '/Users/me/project',  // 新規セッションのフォルダ
  submit: true,              // Enter を押す（エージェントのみ）
});
```

- `ask`（既定）は**送信先…**を出し、利用者に行き先を選ばせます。
- `active` はフォーカス中のペイン、`pane` は `paneId`、`workspace` は `workspaceId` に入力し、`newWorkspace`・`newTab` はそのプロンプトで新しいセッションを始めます。
- ターミナルには常に入力されるだけです。`injectPrompt` が Enter を押すことはありません。
- 60,000 バイトを超えるプロンプトは `~/.agentty/prompts/` に保存され、そのファイルを読むようエージェントに伝えられます。

他のアプリやリンクから始まるものには `ask` を使ってください。

## セッションとターミナル

```js
const session = await plugin.getSession({ paneId: context.pane.id, maxTurns: 200 });
// { agent, sessionId, title, cwd, status, turnCount, turns: [{ role: 'user' | 'assistant', text }] }

await plugin.sendToTerminal({
  paneId: context.pane.id,
  text: 'Summarize what we did.',
  submit: true,
});
```

エージェントに入力する前に `pane.status` を確認してください。`working`、`permission`、`question` は邪魔してはいけません。

## 次に読むもの

- [プロトコル](/docs/plugin-protocol) — SDK なしで同じことをする
- [権限](/docs/plugin-permissions)
