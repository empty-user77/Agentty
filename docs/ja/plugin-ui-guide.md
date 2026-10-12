---
title: パネルのデザイン
description: 何にどの要素を使うか、パネルを Agentty の一部のように見せるいくつかのルール、そのまま出発点にできる画面一式。
---

プラグインは自分では描きません。要素のツリーを送ると、Agentty がアプリと同じ色・書体・余白で描きます。だからどのプラグインも Agentty 本体と同じくらい見栄えよくできます。そのために必要なのは要素を飾ることではなく、合った要素を選ぶことです。

> [!TIP]
> **プラグイン** ページから **UI Gallery** をインストールしてください（Agentty に同梱されています）。すべての要素を実際に動く形で見せ、それぞれの下にその要素を描いたコードを表示します。このページのレシピも、クリックして試せる画面として入っています。

## 用途に合った要素を選ぶ

| 見せたいもの | 使う要素 | 使わないもの |
|---|---|---|
| ユーザーが選ぶもの（ノート、タスク、セッション） | `list` — アイコン、タイトル、サブタイトル、詳細、ホバーで出るボタン | ボタンを縦に並べる |
| 同じ項目を持つ記録（実行、ファイル、リクエスト） | `table` — 見出し、揃った列、クリックで行を選択 | 項目をサブタイトルにつなげた list |
| ひとつのものの詳細 | `card` の中の `keyValue` | コロン入りの `text` を何行も |
| 大事な数字 | `stat`、複数なら `grid` に | `title` のテキスト |
| まとまっているもの | `card`（浮いた箱）か `section`（控えめな見出し） | あらゆる間に `divider` |
| 同じプラグインの複数の画面 | `tabs` — 選ばれたタブの中身だけを送る | 上部の `choice` |
| どこまで進んだか | `progress`、わからなければ `spinner` | テキストのパーセント |
| ヒント、警告、うまくいかなかった理由 | `tone` 付きの `callout` | 段落まるごと `error` スタイルの `text` |
| 設定 | `section` の中の `input`、`select`、`checkbox`、`toggle` | 決まった選択肢を自由入力で |
| コード、コマンド、出力 | `language` 付きの `code` | 複数行に `code` スタイルの `text` |
| 自動化がたどる手順 | `flow` | 番号付きリスト |

## 見た目を整えるルール

1. **primary ボタンは 1 画面に 1 つ。** ユーザーがしに来たことそのものです。ほかは `secondary`、キャンセルなどは `ghost`、`danger` は元に戻せない操作だけに。
2. **まとめて、余白は任せる。** 関連する要素は `card` か `section` に入れ、余白は Agentty に任せてください。余白は `gap` だけで、ほぼどこでも `medium` が合います。
3. **tone には意味がある。** `success`、`warning`、`error` は状態を、`info` は注目してほしいものを示します。飾りに使うと何も伝わらなくなります。
4. **どのリストにも空の状態を、どの呼び出しにもエラーの状態を。** `list` と `table` には次にすることを伝える `empty` を付け、失敗はやり直す手段と一緒に `callout` で見せてください。
5. **処理中であることを見せる。** 何かが始まったらすぐ `spinner` か `progress` を。クリックしても何も変わらないパネルは壊れているように見えます。
6. **短い言葉で。** 「タブで開く」のように短く。詳しいことはタイトルではなくサブタイトルや `detail` に。
7. **パネルは狭い。** 最初は 360 px です。`grid` は 2 列で十分、`table` は 3〜4 列までにしましょう。`row` ではテキストと入力欄が幅を分け合い、ボタンは自分のサイズのままです。
8. **ユーザーの言語で。** `context.language` は `en`、`ko`、`ja`、`zh` のいずれかです。文言は表で管理しましょう。

## レシピ

そのままコピーできる画面一式です。同じコードが UI Gallery の **レシピ** にあります。

### リストと詳細

```js
ui.column([
  ui.row([ui.input('search', { placeholder: 'Search tasks' }), ui.button('new', 'New', { variant: 'primary', icon: 'plus' })]),
  ui.list('tasks', tasks.map((t) => ({ id: t.id, title: t.title, subtitle: t.agent, icon: t.done ? 'circle-check' : 'loader-circle', tone: t.done ? 'success' : 'info' })),
    { empty: 'No tasks yet. Press New to start one.' }),
  picked && ui.card([
    ui.keyValue([
      { label: 'Status', value: picked.done ? 'Done' : 'Working', tone: picked.done ? 'success' : 'info' },
      { label: 'Branch', value: picked.branch, mono: true },
    ]),
    ui.row([ui.button('open', 'Open in a tab', { icon: 'external-link' }), ui.button('delete', 'Delete', { variant: 'ghost' })], { gap: 'small' }),
  ], { title: picked.title, icon: 'file-text' }),
]);
```

### 設定フォーム

```js
ui.column([
  ui.section('Connection', [
    ui.input('url', { placeholder: 'https://api.example.com', value: settings.url }),
    ui.select('env', [{ value: 'dev', label: 'Development' }, { value: 'prod', label: 'Production' }], settings.env),
  ]),
  ui.section('Behavior', [
    ui.toggle('watch', 'Watch for changes', settings.watch),
    ui.checkbox('confirm', 'Ask before sending prompts', settings.confirm, { description: 'Shows the prompt to the user first.' }),
  ]),
  ui.row([ui.button('save', 'Save', { variant: 'primary' }), ui.button('reset', 'Reset', { variant: 'ghost' })], { gap: 'small' }),
]);
```

### ダッシュボード

```js
ui.column([
  ui.grid([
    ui.stat('Open PRs', String(prs.length), { detail: '+2 today', tone: 'info', icon: 'git-pull-request' }),
    ui.stat('CI', ci.ok ? 'passing' : 'failing', { tone: ci.ok ? 'success' : 'error', icon: ci.ok ? 'circle-check' : 'circle-x' }),
  ]),
  ui.section('Recent runs', [
    ui.table('runs', [{ label: 'Workflow', grow: 3 }, { label: 'Result', grow: 2 }, { label: 'Time', align: 'end' }],
      runs.map((r) => ({ id: r.id, cells: [r.name, r.result, r.duration], tone: r.ok ? 'success' : 'error' }))),
  ]),
]);
```

### 実行中のジョブ

```js
ui.card([
  ui.progress(done / total, { label: `Step ${done + 1} of ${total} · ${step}` }),
  ui.code(output, { language: 'sh' }),
  ui.row([ui.button('stop', 'Stop', { variant: 'danger', icon: 'square' })]),
], { title: 'Release check', subtitle: `started ${since}`, icon: 'rocket', tone: 'info' });
```

### 空の状態とエラー

```js
ui.column([
  ui.list('notes', [], { empty: 'No notes yet. Create one to get started.' }),
  ui.callout('Check the token in Settings and try again.', { title: 'Could not sign in', tone: 'error' }),
  ui.row([ui.button('retry', 'Try again', { variant: 'primary', icon: 'refresh-cw' })]),
]);
```

## 要素

すべてのビルダーのオプションとイベントは [Node.js SDK](/docs/plugin-sdk#パネル) に、各要素が送る JSON は [プロトコル](/docs/plugin-protocol#ui-ツリー) にあります。まとめると：

| 要素 | イベント |
|---|---|
| `column`、`row`、`grid`、`section`、`card`、`divider` | |
| `tabs` | タブの id 付きで `change` |
| `text`、`badge`、`keyValue`、`stat`、`code`、`callout`、`progress`、`spinner` | |
| `list` | `item` 付きで `select`、行のボタンは `item` と `action` 付きで `action` |
| `table` | `item`（行の id）付きで `select` |
| `flow` | `item`（手順の id）付きで `select` |
| `button` | `click` |
| `input` | 入力が止まると `change`、Enter で `submit` |
| `select`、`choice` | 選択肢の値付きで `change` |
| `checkbox`、`toggle` | 新しい真偽値付きで `change` |
| `popover` | `close` |

`card`、`grid`、`tabs`、`table`、`keyValue`、`stat`、`progress`、`callout`、`select`、`checkbox`、`code` は API バージョン 4 です。使うときはマニフェストに `"apiVersion": 4` と書いてください。古い Agentty は描けないパネルを出す代わりにアップデートを促します。両方で動かしたいプラグインは、`host/info` の `uiFeatures` で実行中の Agentty が描ける要素を確かめられます。

## 次に読むもの

- [Node.js SDK](/docs/plugin-sdk) · [Rust と WebAssembly](/docs/plugin-rust)
- [プラグインプロトコル](/docs/plugin-protocol) — JSON としての UI ツリーと、その上限
