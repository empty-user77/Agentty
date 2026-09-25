---
title: 插件快速开始
description: 用 AI、用 Rust 或用 JavaScript 做一个能跑的 Agentty 插件，并把它装上。
---

插件是一个装着清单和程序的文件夹。它可以在终端旁边放一个**面板**，在智能体窗格上方加**按钮**，往**命令面板**里添条目，也可以把文本作为**提示词**交给智能体。

有两种，值得先选好：

| | |
|---|---|
| **WebAssembly**（`runtime: "wasm"`） | 一个 `.wasm` 文件，通常用 Rust 构建。Agentty 自己运行它，所以它只能碰到协议给的东西，用户机器上也不需要装什么。这是[插件市场](/docs/plugin-publishing)唯一收录的形式。 |
| **程序**（`node`、`python`、`executable`） | 以用户身份运行，拥有和用户启动的任何东西一样的访问权限。写起来快，从用户自己挑的文件夹或 Git 仓库安装。 |

## 用 AI 做

大多数插件是这么写出来的。从活动栏的拼图图标打开**插件**，在*自己动手*下填上名字和这个插件要做什么，然后按**用 Claude Code 创建并构建**。

Agentty 会从模板创建插件——SDK、类型定义、开发者指南，以及带说明的 `CLAUDE.md` / `AGENTS.md`——并在该文件夹里打开 Claude Code。按**复制 AI 提示词**可以把同一段提示词用在别的智能体上。

### 该跟智能体说什么

模板本身已经带着指南了，所以真正有用的提示词是关于插件本身，而不是平台。

```text
先读 PLUGIN_GUIDE.md 和 agentty-plugin.d.ts。

做一个面板，列出当前聚焦窗格所在文件夹里的 Git 分支。
每一行显示分支名，以及它相对 main 领先或落后多少。
点击某一行时，请智能体总结这个分支上改了什么。

只申请 prompt.inject 和 workspace.read，别的都不要。
```

然后让它检查代码（`node --check main.mjs`，或 `cargo build --release --target wasm32-unknown-unknown`），再到 Agentty 的插件卡片上按**重启**加载。出错的话看同一张卡片上的**日志**。

> [!TIP]
> 如果这段提示词是写给模板文件夹之外的智能体，请把[清单参考](/docs/plugin-manifest)、[UI 树](/docs/plugin-protocol#ui)和[协议](/docs/plugin-protocol)指给它。这三页就是全部。

## 手写 —— Rust

```
hello/
├── agentty-plugin.json   清单
├── hello.wasm            编译好的模块
└── src/lib.rs            它的源码
```

```rust
use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};

#[derive(Default)]
struct Hello {
    clicks: u32,
}

impl Plugin for Hello {
    fn panel_open(&mut self, host: &Host) {
        host.set_panel(ui::column(vec![
            ui::text(format!("Clicked {} times", self.clicks)),
            ui::button("go", "Click me"),
        ]));
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        if event.element == "go" {
            self.clicks += 1;
            self.panel_open(host);
        }
    }
}

export_plugin!(Hello);
```

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "runtime": "wasm",
  "main": "hello.wasm",
  "contributes": { "panel": { "title": "Hello", "surface": "sidebar" } }
}
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

完整的 SDK 在 [Rust 与 WebAssembly](/docs/plugin-rust)。

## 手写 —— JavaScript

```
~/.agentty/plugins/hello/
├── agentty-plugin.json   清单
├── main.mjs              插件程序
└── agentty-plugin.mjs    SDK
```

SDK 是一个没有依赖的单文件。在插件页面按**开发者指南**，Agentty 会把它连同类型和文档一起解压到 `~/.agentty/plugins/.sdk/`，从那里把 `agentty-plugin.mjs` 复制到 `main.mjs` 旁边。

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [{ "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }]
  }
}
```

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();

plugin
  .onPanelOpen((context) =>
    plugin.setPanel(
      ui.column([
        ui.text('Hello', 'title'),
        ui.text(context.pane ? `You are in ${context.pane.cwd}` : 'No terminal focused', 'muted'),
        ui.button('explain', 'Explain this folder', { icon: 'bot', variant: 'primary' }),
      ]),
    ),
  )
  .onEvent('explain', (_event, context) => explain(context))
  .command('hello.explain', ({ context }) => explain(context))
  .start();

function explain(context) {
  return plugin.injectPrompt({ text: 'Give me a short tour of this project.', cwd: context.pane?.cwd, target: 'ask' });
}
```

> [!WARNING]
> 千万不要往 stdout 写东西（`console.log`）——协议走的就是那条通道。日志请用 `plugin.log()` 或 `console.error()`。

其余内容在 [Node.js SDK](/docs/plugin-sdk)。

## 安装

**插件 → 从文件夹安装…** 选中文件夹，或者用**链接开发文件夹…** 让它在原地运行。然后**刷新**（或重新打开页面）：插件就会显示为已安装，面板按钮出现在 `surface` 指定的位置，命令出现在命令面板（⇧⌘P）中。

## 测试

`node`、`python` 和 `executable` 插件就是读 stdin、写 stdout 的普通程序，所以不用 Agentty 也能驱动：

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"hello","dataDir":"/tmp"},"context":{}}}' \
  '{"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}' \
  | node main.mjs
```

它应该先回应 `initialize`，然后发出一个 `ui/setPanel` 请求。

## 下一步

- [清单参考](/docs/plugin-manifest) —— 每一个字段
- [Rust 与 WebAssembly](/docs/plugin-rust) · [Node.js SDK](/docs/plugin-sdk)
- [AgentOS 插件](/docs/plugin-agentos) —— 通过智能体推进工作
- [权限](/docs/plugin-permissions) · [发布](/docs/plugin-publishing)
