---
title: Node.js SDK
description: 处理器、调用、面板 UI 构建器与上下文对象 —— agentty-plugin.mjs 的全部能力。
---

`agentty-plugin.mjs` 是一个无依赖的单文件，类型定义在同目录的 `agentty-plugin.d.ts` 中。在插件页面点击**开发者指南**，Agentty 会把两者解压到 `~/.agentty/plugins/.sdk/`。

> [!NOTE]
> 这样写出来的插件会在用户机器上以程序方式运行，需要 `PATH` 里有 Node.js 18 及以上，并且从文件夹或 Git 仓库安装。[插件市场](/docs/plugin-publishing)只收录 WebAssembly 模块——那条路见 [Rust 与 WebAssembly](/docs/plugin-rust)。

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();
// 注册处理器…
plugin.start();
```

所有处理器都要在调用 `start()` 之前注册。

## 处理器

处理器都可以是 async 的。错误会被记录，并作为通知显示给用户。

| 处理器 | 触发时机 |
|---|---|
| `onActivate(info => …)` | 插件已启动；`info` 中有 `plugin.dataDir`、`language`、`context` |
| `command(id, ({ context, args }) => …)` | 命令从命令面板被执行 |
| `onPanelOpen(context => …)` | 面板变为可见 —— 在这里绘制 |
| `onPanelClose(context => …)` | 面板被隐藏 |
| `onEvent(elementId, (event, context) => …)` | 该 id 的 UI 元素被操作 |
| `onAnyEvent((event, context) => …)` | `onEvent` 未处理的所有 UI 事件 |
| `onContextChange(context => …)` | 聚焦窗格、其状态或文件夹发生变化 |
| `onUrl(path, ({ path, query, url }) => …)` | `agentty://plugin/<id>/<path>?…` 被打开 |
| `onShutdown(() => …)` | Agentty 正在停止插件 |

## 调用

所有调用都返回 promise。

| 调用 | 权限 |
|---|---|
| `setPanel(tree)` —— 替换面板内容 | |
| `showPanel()` —— 打开本插件的面板 | |
| `notify(message, kind)` —— `info`、`success`、`warning`、`error` | |
| `setBadge(text)` —— 标签栏按钮上最多 8 个字符 | |
| `getContext()` | |
| `openUrl(url)` —— http/https | |
| `copy(text)` —— 复制到剪贴板 | |
| `revealPath(path)` —— 在文件管理器中显示 | `workspace.read` |
| `injectPrompt(request)` | `prompt.inject` |
| `sendToTerminal({ paneId, text, submit })` | `terminal.write` |
| `getSession({ paneId, maxTurns })` | `session.read` |
| `listWorkspaces()` | `workspace.read` |
| `fetch(request)` —— HTTP 请求 | `net.request` |
| `log(...)` —— 写入插件日志（stderr） | |

协议里还有 `storage/get`、`storage/set` 和 `storage/keys`（插件自己文件夹里的一个 JSON 文档，不需要权限），以及从 API 版本 2 起的 `host/timer` 和 `pane/status`。Node 插件也可以自己写文件，但 storage 对两种插件的行为是一样的。见[协议](/docs/plugin-protocol)。

`plugin.info` 保存 `initialize` 的数据，`plugin.context` 是最新的上下文。

## 面板

面板宽 360px，可纵向滚动。插件用一棵树来描述它，由 Agentty 原生绘制，因此外观与应用一致，也不需要网页视图。任何变化时发送一棵新树即可；除非你发送不同的 `value`，文本框会保留用户输入的内容。

| 构建器 | 元素 | 事件 |
|---|---|---|
| `ui.column(children, { gap })` / `ui.row(children, { gap, wrap })` | 布局。`gap`: `none`、`small`、`medium`、`large` | |
| `ui.section(title, children)` | 带标题的分组 | |
| `ui.text(text, style)` | `body`、`title`、`muted`、`small`、`code`、`error`、`success` | |
| `ui.button(id, label, { icon, variant, disabled })` | `primary`、`secondary`、`ghost`、`danger` | `click` |
| `ui.input(id, { placeholder, value, rows })` | 单行输入；`rows` 大于 1 时为该行数的文本域（最多 24） | 停止输入后 `change`，回车时 `submit`；`event.value` 为文本 |
| `ui.list(id, items, { empty })` | 行 `{ id, title, subtitle, detail, icon, tone, actions }` | 带 `event.item` 的 `select`；行内按钮发送带 `event.item` 和 `event.action` 的 `action` |
| `ui.choice(id, [{ value, label }], value)` | 分段选择 | 带取值的 `change` |
| `ui.toggle(id, label, value)` | 开关 | 带新布尔值的 `change` |
| `ui.badge(text, tone)` | `neutral`、`info`、`success`、`warning`、`error` | |
| `ui.spinner(text)` | | |
| `ui.divider()` | | |

null 与 false 的子元素会被跳过，因此 `条件 && ui.text('…')` 可以直接使用。

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
> 限制：2,000 个元素、12 层深度、每个字符串 20,000 字符；`choice` 的选项和列表项的按钮也计入元素。面板最快每 50ms 重绘一次，通知最快每 700ms 一条。每秒发送超过 240 条消息的插件会被视为失控并停止。

## 上下文

每个命令、事件和面板调用都会带上聚焦窗口的上下文：

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
  "language": "zh"
}
```

能看到多少取决于你声明的权限。文件夹与名称字段（`workspace.cwd`、`workspace.name`、`pane.cwd`、`pane.title`）需要 `workspace.read`，`pane.sessionId` 需要 `session.read`。没有这些权限时，上下文仍带有 id、`kind`、`tool`、`status`、`running` 和语言：足以知道哪个窗格处于焦点，但不知道用户在哪里工作。

`kind` 为 `claude`、`codex` 或 `shell`；其他智能体 CLI 运行在 `shell` 窗格中，由 `tool` 标明名称。

| `status` | 含义 |
|---|---|
| `idle` | 等待输入 |
| `working` | 正在运行工具 |
| `thinking` | 一轮进行中，处于工具调用之间 |
| `finished` | 本轮结束 |
| `permission` | 正在请求许可 |
| `question` | 正在向用户提问 |
| `interrupted` | 被用户中断 |
| `shell` | 普通 shell |
| `exited` | 程序已退出 |

判断是否打扰时，把 `working` 和 `thinking` 同等看待。

## 发送提示词

```js
await plugin.injectPrompt({
  text: 'Continue the release checklist.',
  title: 'Release',          // 新会话的工作区名称，也是对话框标题
  target: 'ask',             // ask | active | newWorkspace | newTab | pane | workspace
  agent: 'claude',           // 新会话使用的 claude | codex | shell
  cwd: '/Users/me/project',  // 新会话的文件夹
  submit: true,              // 按回车（仅限智能体）
});
```

- `ask`（默认）会弹出**发送到…**，由用户选择目的地。
- `active` 输入到聚焦窗格，`pane` 输入到 `paneId`，`workspace` 输入到 `workspaceId`，`newWorkspace` / `newTab` 则用该提示词开启新会话。
- 终端永远只会被输入文本 —— `injectPrompt` 不会按回车。
- 超过 60,000 字节的提示词会保存到 `~/.agentty/prompts/`，并让智能体去读取该文件。

凡是由其他应用或链接发起的，请使用 `ask`。

## 会话与终端

```js
const session = await plugin.getSession({ paneId: context.pane.id, maxTurns: 200 });
// { agent, sessionId, title, cwd, status, turnCount, turns: [{ role: 'user' | 'assistant', text }] }

await plugin.sendToTerminal({
  paneId: context.pane.id,
  text: 'Summarize what we did.',
  submit: true,
});
```

向智能体输入前请检查 `pane.status`：不要打断 `working`、`permission` 或 `question`。

## 接下来

- [协议](/docs/plugin-protocol) —— 不用 SDK 实现同样的能力
- [权限](/docs/plugin-permissions)
