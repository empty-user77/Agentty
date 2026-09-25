<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="100%">
</p>

<p align="center"><b>面向 AI 原生开发的编排终端</b></p>

<p align="center">
  <a href="https://www.agentty.run">agentty.run</a> ·
  <a href="https://www.agentty.run/docs">文档</a> ·
  <a href="https://github.com/empty-user77/agentty-releases/releases">下载</a> ·
  <a href="https://x.com/agentty_run">@agentty_run</a>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.ko.md">한국어</a> ·
  <a href="README.ja.md">日本語</a> ·
  <b>中文</b>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="Platform: macOS | Windows | Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey.svg">
</p>

Agentty 是一个原生、GPU 渲染的终端，让 **Claude Code**、**Codex** 和你的各种 shell 并排运行。它会显示哪个智能体
正在工作、哪个已经完成、哪个在等你回答，并把工作真正需要的东西交给它们：项目的文件、Git 历史、容器和数据库。

使用 Rust、[GPUI](https://gpui.rs) 和 `alacritty_terminal` 编写，不含 Electron。

## 为什么选择 Agentty

- **所有智能体都在一个窗口里** — 工作区、标签页和分屏在重启后依然还原。终端、Claude Code、Codex，以及 Gemini、
  Copilot、Cursor、OpenCode、Amp、Aider 和本地 Ollama 模型，都从同一个菜单启动。
- **能分辨它们的状态** — 每个分屏和工作区都实时显示：工作中、已完成、等待你回答。需要你的那个分屏会亮起边框，
  通知会出现在桌面和菜单栏；你不在电脑前时，还可以发到 Slack、Discord 或 Telegram。
- **会话之间可以传递上下文** — 在两个智能体之间拉一条连线即可交接对话，保持双向同步，或在 Claude Code 与 Codex
  之间迁移会话。
- **并行工作而不互相干扰** — 同一项目中的第二个智能体会询问是在新分支的独立 git worktree 中启动，还是共用现有的；智能体请求把工作拆成
  多个会话时，经你确认后一次性开始。
- **不只是终端** — 带编辑器的文件面板、GitHub Desktop 风格的 Git 页面、Docker 面板、数据库页面、应用内浏览器、
  用量与成本监控，以及汇总技能、子智能体和 MCP 服务器的扩展页面。
- **谨慎对待你的数据** — 智能体的查询在只读事务中执行，任何写入都必须通过显示原始语句的确认对话框；密钥保存在
  系统凭据库中；对话记录和提示词不会离开本机。

完整的功能说明见 **[agentty.run/docs](https://www.agentty.run/docs)**。

## 安装

从 [agentty-releases](https://github.com/empty-user77/agentty-releases/releases) 下载：

| 平台 | 文件 |
|---|---|
| macOS 13+（Apple Silicon） | `Agentty-X.Y.Z-…-arm64.dmg` — 已签名并公证 |
| Windows 10 1809+（x64） | `Agentty-X.Y.Z-windows-x64-setup.exe` — 按用户安装，无需管理员权限 |
| Debian 12+ / Ubuntu 22.04+ | `Agentty-X.Y.Z-linux-amd64.deb` — `sudo apt install ./Agentty-*.deb` |
| RHEL 9+ / Fedora | `Agentty-X.Y.Z-linux-x86_64.rpm` — `sudo dnf install ./Agentty-*.rpm` |

Agentty 会自行更新：在 macOS 和 Windows 上安装新版本并重启，在 Linux 上指向新的软件包。

### 从源码构建

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

需要 Rust 1.98（在 `rust-toolchain.toml` 中固定）和各平台的构建工具；Windows 与 Linux 请见
[docs/platforms.md](docs/platforms.md)。`PATH` 中有 `claude` 或 `codex` 即可使用智能体标签页，缺少的工具可在
设置 → 环境检查 中安装。

## 文档

| | |
|---|---|
| [功能指南](https://www.agentty.run/docs) | 应用各个部分的说明 |
| [docs/configuration.md](docs/configuration.md) | `settings.json`、智能体登录、harness、数据库、主题，以及 Agentty 生成的文件 |
| [docs/platforms.md](docs/platforms.md) | Windows 与 Linux 的差异及构建方式 |
| [docs/plugins](docs/plugins/README.md) | 编写插件（Node.js SDK 或[协议](docs/plugins/protocol.md)）与[使用方法](docs/plugins/usage.md) |
| [docs/architecture.md](docs/architecture.md) | crate、终端、智能体套接字与各面板的结构 |
| [docs/metrics.md](docs/metrics.md) | 官方构建可能发送的全部事件，以及如何关闭 |
| [docs/release.md](docs/release.md) | 打包、签名、公证与发布渠道 |

## 快捷键

| 操作 | 快捷键 |
|---|---|
| 新建终端 / Claude Code / Codex 标签页 | ⌘T / ⌥⌘C / ⌥⌘X |
| 向右 / 向下分屏 | ⌘D / ⇧⌘D |
| 下一个·上一个 分屏 · 标签页 · 工作区 | ⌘] ⌘[ · ⇧⌘] ⇧⌘[ · ⌥⌘↓ ⌥⌘↑ |
| 命令面板 · 跳到未读 | ⇧⌘P · ⇧⌘U |
| Git · 会话连接 · 监控 · 设置 | ⇧⌘G · ⇧⌘F · ⌥⌘U · ⌘, |
| 文件面板 · 应用内浏览器 · 迷你模式 | ⌥⌘B · ⇧⌘B · ⌃⌘M |

完整列表见 **设置 → 键盘快捷键**。在 Windows 和 Linux 上，⌘ 对应 Ctrl+Shift，⇧⌘ 对应 Ctrl+Alt+Shift，
⌥⌘ 对应 Ctrl+Alt，因此 Ctrl+字母 仍然交给 shell。

## 插件

插件会填充终端旁边的面板，在智能体分屏上方添加按钮、在命令面板中添加条目，并可以发送提示词 — 发送到哪里，始终由你
在"选择发送位置"对话框中决定。插件请求的权限在安装前会显示在插件页面上，它们限制插件能请求 Agentty 做的事。WebAssembly 插件无法接触
这些权限之外的任何东西。以 Node.js、Python 或原生程序运行的插件在 Agentty 之外拥有和你相同的文件与网络访问权限，
所以只安装你信任的插件。插件通过 [Agentty Marketplace](https://github.com/empty-user77/Agentty-Marketplace) 发布。内置的
**Cosmica** 插件会把 [Cosmica](https://www.cosmica.ink/) 笔记变成提示词，并把会话摘要写回去。

详见 [docs/plugins](docs/plugins/README.md)。你也可以在插件页面新建一个插件，让 Claude Code 来实现它。

## 隐私

为了列出会话、统计用量和传递上下文，Agentty **只在本地** 读取智能体的对话记录。提示词、输出、路径和仓库数据不会
上传，智能体状态钩子只与你账户私有的套接字通信。

官方发布版本会发送匿名使用统计（使用了哪些功能、应用与系统版本、随机安装 ID）。可在
设置 → 通用 中关闭，`DO_NOT_TRACK=1` 同样会关闭；从源码构建的版本不发送任何内容。全部事件列在
[docs/metrics.md](docs/metrics.md) 中。

## 参与贡献

欢迎贡献 — 请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 和[行为准则](CODE_OF_CONDUCT.md)。安全问题请见
[SECURITY.md](SECURITY.md)。

## 许可证

```
Copyright 2026 LEE YONGBEOM

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

   http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

完整文本见 [LICENSE](LICENSE)。

内置字体与第三方组件遵循各自的许可证，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
