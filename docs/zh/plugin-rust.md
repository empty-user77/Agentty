---
title: Rust 与 WebAssembly
description: 用 Rust 写插件，打包成一个到处都能跑的 .wasm 文件，并且只能碰到协议交给它的东西。
---

把 `runtime` 设为 `"wasm"`，`main` 就是一个 Agentty **在自己内部**用解释器运行的 WebAssembly 模块。一个文件在 macOS、Windows 和 Linux 上都能跑，用户机器上不需要装任何东西——不需要 Node.js，也不需要 Python。

它也是[插件市场](/docs/plugin-publishing)唯一接受的形式。之所以能这样，是因为模块只能碰到协议交给它的东西。

## 模块能碰到什么

Agentty 交给模块的函数只有三个：

| 从 `agentty` 模块 import | |
|---|---|
| `send(ptr, len)` | 向 Agentty 发送一条 UTF-8 JSON 消息 |
| `log(ptr, len)` | 写入插件日志的一行 |
| `now_ms() -> i64` | Unix 纪元以来的毫秒数 |

没有文件，没有套接字，没有环境变量，没有进程，除了那个计数器之外也没有时钟。WebAssembly 插件**不管怎么写**都读不到 `~/.agentty`、你的项目或你的凭据——不是因为它承诺不读，而是因为它从来就没拿到那样的函数。import 了别的东西的模块根本不会被加载。

其余一切它都用与进程型插件写到 stdout 相同的 JSON-RPC 消息向 Agentty 请求，`agentty-plugin.json` 里的[权限](/docs/plugin-permissions)也照样检查。

> [!NOTE]
> `runtime` 为 `node`、`python` 或 `executable` 的插件正好相反：它以你的身份运行，拥有与你启动的任何程序相同的访问权限。插件页面的**关于 → 运行方式**会说明是哪一种。

## SDK

Rust SDK 在[插件市场仓库](https://github.com/empty-user77/Agentty-Marketplace/tree/main/sdk/rust)的 `sdk/rust` 下，旁边就是用它写成的插件。

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
agentty-plugin = { path = "../../sdk/rust" }   # 或指向该仓库的 git 依赖

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true
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

## `Host` 提供什么

| | |
|---|---|
| `set_panel(tree)` · `show_panel()` | 用 `ui::` 构建的面板 |
| `notify_user(kind, message)` · `set_badge(text)` | 提示，以及图标上最多 8 个字符 |
| `copy(text)` | 复制到剪贴板 |
| `log(line)` | 插件页面上的日志 |
| `open_url(url)` | 在用户浏览器中打开页面 |
| `fetch(request)` | HTTP 请求——需要 `net.request` |
| `prompt(text, target)` | 发给智能体的提示词——需要 `prompt.inject` |
| `call(method, params)` · `notify(method, params)` | 协议中的其余一切 |
| `context()` · `now_ms()` | 用户当前所在位置，以及时钟 |

`call` 和 `fetch` 返回一个请求 id，响应会到达 `Plugin::answer`。

模块没有自己的文件，所以在两次运行之间记住东西的办法是 `storage/get`、`storage/set` 和 `storage/keys`：它自己文件夹里的一个 JSON 文档，最多 64 个键、共 1 MB，不需要权限。

## 构建与安装

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

把模块放在 `agentty-plugin.json` 旁边，然后**插件 → 从文件夹安装…** 选中该文件夹。重新构建后，插件页面上的**重启**会加载新版本。

## 徽标

模块没有可以放图片的文件夹，于是模块自己带着图片：一个名为 `agentty.logo` 的 WebAssembly 自定义段。引擎会忽略这个段，而市场条目里的校验和本来就覆盖它 —— 徽标和模块的其余部分一样，都是经过审核的字节。在 Rust 中这就是一个 static：

```rust
#[used]
#[link_section = "agentty.logo"]
static LOGO: [u8; 1234] = *include_bytes!("logo.png");
```

`#[used]` 不能省：发布构建会丢掉没有任何东西引用的 static，段也会随之消失。数组长度必须等于文件长度。

这些字节必须是不超过 512 KB 的 PNG、JPEG、GIF 或 WebP，安装时会据此核对。SVG 无论起什么名字都会被拒绝 —— 见[徽标](/docs/plugin-manifest#徽标)。Agentty 会在安装时把图片写成文件，因此绘制列表时不会解析模块。

## 限制

| | |
|---|---|
| 模块大小 | 64 MB（市场条目为 8 MB） |
| 内存 | 64 MB |
| 单条消息 | 16 MB |
| 每条消息的工作量 | 有预算——不返回的插件会以「未能及时完成」停止 |
| 处理一条消息期间发出的消息 | `send` 与 `log` 合计超过 256 条即停止 |

一次只处理一条消息。模块**只在**处理消息时运行，所以没有后台循环：`host/timer` 是一个到点才被响应的请求，而插件得到的也就只有那个响应。

## 示例

两个都在插件市场仓库里，连源码一起，都可以从**插件 → 市场**安装：

- [`hello-rust`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/hello-rust) —— 一个面板和一个计数器，不要求任何权限。
- [`agent-rest-client`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/agent-rest-client) —— 带环境变量、已保存请求和代理的 HTTP 客户端，使用 `net.request`。

## 下一步

- [AgentOS 插件](/docs/plugin-agentos) —— 通过智能体推进工作的插件
- [清单参考](/docs/plugin-manifest) · [插件协议](/docs/plugin-protocol) —— 含模块 ABI
- [权限](/docs/plugin-permissions) · [发布](/docs/plugin-publishing)
