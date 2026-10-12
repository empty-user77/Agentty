---
title: 设计面板
description: 什么内容用哪个元素、让面板看起来像 Agentty 一部分的几条规则，以及可以直接拿来起步的完整界面。
---

插件自己不绘制：它发送一棵元素树，由 Agentty 用应用自身的颜色、字体和间距来绘制。所以任何插件都可以和 Agentty 本身一样好看——做到这一点靠的是选对元素，而不是去装饰元素。

> [!TIP]
> 在 **插件** 页面安装 **UI Gallery**（随 Agentty 附带）。它实时展示每个元素，并在每个元素下方显示绘制它的代码；本页的模板也以可以点击试用的界面形式收录其中。

## 按用途选择元素

| 要展示的内容 | 使用 | 不要用 |
|---|---|---|
| 供用户挑选的条目（笔记、任务、会话） | `list` — 图标、标题、副标题、详情、悬停时出现的按钮 | 竖排一列按钮 |
| 字段相同的记录（运行、文件、请求） | `table` — 表头、对齐的列、点击选中一行 | 把字段拼进副标题的 list |
| 某一项的详细信息 | `card` 里的 `keyValue` | 好几行带冒号的 `text` |
| 重要的数字 | `stat`，多个时放进 `grid` | `title` 文本 |
| 属于同一组的内容 | `card`（浮起的方框）或 `section`（低调的标题） | 到处都是 `divider` |
| 同一插件的多个视图 | `tabs`，只发送选中标签页的内容 | 顶部放一个 `choice` |
| 进行到哪一步 | `progress`；说不准时用 `spinner` | 文本里的百分比 |
| 提示、警告、出错原因 | 带 `tone` 的 `callout` | 整段都用 `error` 样式的 `text` |
| 设置 | `section` 里的 `input`、`select`、`checkbox`、`toggle` | 用自由输入代替固定选项 |
| 代码、命令、输出 | 带 `language` 的 `code` | 多行内容用 `code` 样式的 `text` |
| 自动化依次经过的步骤 | `flow` | 编号列表 |

## 让面板好看的规则

1. **每个界面只有一个 primary 按钮。** 它就是用户来这里要做的事。其余用 `secondary`，取消之类用 `ghost`，`danger` 只用于无法撤销的操作。
2. **先分组，间距交给 Agentty。** 把相关元素放进 `card` 或 `section`，间距交给 Agentty——间距只有 `gap` 一种，几乎所有地方用 `medium` 都合适。
3. **tone 是有含义的。** `success`、`warning`、`error` 表示状态，`info` 提醒注意。拿来做装饰，它就什么也表达不了。
4. **每个列表都要有空状态，每次调用都要有错误状态。** 给 `list` 和 `table` 一段告诉用户下一步做什么的 `empty` 文字，失败时用 `callout` 展示并给出重试的办法。
5. **让人看到正在处理。** 一开始处理就显示 `spinner` 或 `progress`——点击后毫无变化的面板看起来像坏了。
6. **用词简短。** 例如“在标签页中打开”。详细内容放在副标题或 `detail` 里，不要塞进标题。
7. **面板很窄。** 初始宽度是 360 px：`grid` 两列就够了，`table` 最多三四列；`row` 中文本和输入框分享宽度，按钮保持自身大小。
8. **使用用户的语言。** `context.language` 是 `en`、`ko`、`ja` 或 `zh` 之一；文案放在一张表里管理。

## 模板

可以直接复制的完整界面。UI Gallery 的 **模板** 里有同样的代码。

### 列表与详情

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

### 设置表单

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

### 仪表盘

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

### 正在运行的任务

```js
ui.card([
  ui.progress(done / total, { label: `Step ${done + 1} of ${total} · ${step}` }),
  ui.code(output, { language: 'sh' }),
  ui.row([ui.button('stop', 'Stop', { variant: 'danger', icon: 'square' })]),
], { title: 'Release check', subtitle: `started ${since}`, icon: 'rocket', tone: 'info' });
```

### 空状态与错误

```js
ui.column([
  ui.list('notes', [], { empty: 'No notes yet. Create one to get started.' }),
  ui.callout('Check the token in Settings and try again.', { title: 'Could not sign in', tone: 'error' }),
  ui.row([ui.button('retry', 'Try again', { variant: 'primary', icon: 'refresh-cw' })]),
]);
```

## 元素

每个构建函数的选项和事件见 [Node.js SDK](/docs/plugin-sdk#面板)，每个元素发送的 JSON 见 [协议](/docs/plugin-protocol#ui)。简要如下：

| 元素 | 事件 |
|---|---|
| `column`、`row`、`grid`、`section`、`card`、`divider` | |
| `tabs` | 带标签页 id 的 `change` |
| `text`、`badge`、`keyValue`、`stat`、`code`、`callout`、`progress`、`spinner` | |
| `list` | 带 `item` 的 `select`；行按钮发送带 `item` 和 `action` 的 `action` |
| `table` | 带 `item`（行 id）的 `select` |
| `flow` | 带 `item`（步骤 id）的 `select` |
| `button` | `click` |
| `input` | 停止输入后 `change`，按 Enter 时 `submit` |
| `select`、`choice` | 带选项值的 `change` |
| `checkbox`、`toggle` | 带新布尔值的 `change` |
| `popover` | `close` |

`card`、`grid`、`tabs`、`table`、`keyValue`、`stat`、`progress`、`callout`、`select`、`checkbox` 和 `code` 属于 API 版本 4：使用时请在清单中写上 `"apiVersion": 4`，较旧的 Agentty 就会提示用户更新，而不是显示一个画不出来的面板。想同时兼容两者的插件，可以通过 `host/info` 的 `uiFeatures` 查看当前运行的 Agentty 能绘制哪些元素。

## 接下来

- [Node.js SDK](/docs/plugin-sdk) · [Rust 与 WebAssembly](/docs/plugin-rust)
- [插件协议](/docs/plugin-protocol) — 以 JSON 表示的 UI 树及其限制
