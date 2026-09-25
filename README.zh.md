<h1 align="center">
  <a href="https://www.agentty.run"><img src="docs/assets/logo.png" alt="Agentty" width="64" valign="middle" /></a> Agentty
</h1>

<p align="center">
  <a href="https://github.com/empty-user77/Agentty"><img src="https://img.shields.io/github/stars/empty-user77/Agentty?style=flat&amp;label=%E2%98%85&amp;color=08C" alt="GitHub stars" /></a>
  <a href="https://github.com/empty-user77/Agentty/releases"><img src="https://img.shields.io/github/downloads/empty-user77/Agentty/total?style=flat&amp;color=08C" alt="Downloads" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-08C?style=flat" alt="License: Apache-2.0" /></a>
  <a href="https://x.com/agentty_run"><img src="https://img.shields.io/badge/X-000000?logo=x&amp;logoColor=white" alt="Follow Agentty on X" /></a>
  <img src="https://img.shields.io/badge/macOS%20%7C%20Windows%20%7C%20Linux-4493F8?style=flat-square" alt="Supported platforms: macOS, Windows and Linux" />
</p>

<p align="center">
  <sub><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <a href="README.ja.md">日本語</a> · <b>中文</b></sub>
</p>

<p align="center">
  <strong>AI 原生开发工作流的编排终端。</strong><br/>
  让 Claude Code、Codex 和十几个其他智能体并排运行 — 一眼看清谁在工作、谁已完成、谁在等你回答。
</p>

<h3 align="center"><a href="https://github.com/empty-user77/Agentty/releases/latest"><ins>下载 Agentty</ins></a> · <a href="https://www.agentty.run/docs"><ins>文档</ins></a></h3>

<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="960" />
</p>

一个原生、GPU 渲染的终端，使用 Rust 编写，搭配 [GPUI](https://gpui.rs) 和 `alacritty_terminal`。不含 Electron。
它为每个智能体提供工作所需的一切：项目的文件、Git 历史、容器和数据库。

## 功能

<table>
<tr>
<td width="50%" valign="top">

### 所有智能体，一个窗口

工作区、标签页和分屏为你的终端、Claude Code、Codex 和任何其他 CLI 智能体服务 — 重启后全部恢复。

[文档 →](https://www.agentty.run/docs/workspaces-tabs-panes)

</td>
<td width="50%" valign="top">

### 知道谁需要你

每个分屏和工作区都实时显示：智能体在工作、已完成还是在等你回答。通知可以到达你的桌面、菜单栏，或发送到 Slack、Discord 和 Telegram。

[文档 →](https://www.agentty.run/docs/agent-status)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### Session Flow

在两个智能体之间拉一条线即可交接上下文，保持双向同步，或在 Claude Code 与 Codex 间迁移对话。

[文档 →](https://www.agentty.run/docs/session-flow)

</td>
<td width="50%" valign="top">

### 并行工作树

同一项目中的第二个智能体可以在新分支上启动到它自己的 git 工作树，或加入现有的工作树 — 你说了算。智能体也可以请求把工作拆成多个会话，经你确认后一次性启动。

[文档 →](https://www.agentty.run/docs/agent-git)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### AgentGit

切换仓库和分支、查看 diff、提交选定文件、浏览历史、获取、拉取、推送和合并 — 不用离开运行智能体的窗口。

[文档 →](https://www.agentty.run/docs/agent-git)

</td>
<td width="50%" valign="top">

### 应用内浏览器

开发服务器在终端旁边打开，支持手机和平板尺寸，智能体也可以驱动页面来测试它构建的内容。

[文档 →](https://www.agentty.run/docs/in-app-browser)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 数据库和 Docker

浏览你项目已配置的数据库。智能体在只读事务中读取，任何写入都必须由你批准（显示具体的语句）。容器、端口和日志就在旁边。

[文档 →](https://www.agentty.run/docs/databases)

</td>
<td width="50%" valign="top">

### 插件和自动化

插件可以添加面板、按钮和调色板命令，引入其他应用，并在自己的工作区中运行自动化智能体。在 [Marketplace](https://github.com/empty-user77/Agentty-Marketplace) 中找到它们或自己构建。

[文档 →](https://www.agentty.run/docs/plugins-overview)

</td>
</tr>
</table>

**还有其他功能：**

- **[会话](https://www.agentty.run/docs/sessions)** — 浏览、搜索和恢复你电脑上已有的 Claude Code 和 Codex 会话。
- **[监控](https://www.agentty.run/docs/monitoring)** — 从本地文件记录、运行中的 AI 进程、电脑上所有 git 工作树获取成本、令牌和缓存命中率，以及清理构建输出的磁盘页面。
- **[扩展、MCP 和连接器](https://www.agentty.run/docs/mcp-and-connectors)** — 技能、子智能体、命令和 Claude Code 与 Codex 的 MCP 服务器集中管理，密钥由操作系统保管。
- **[构建我的想法](https://www.agentty.run/docs/build-my-idea)** 和 **[启动](https://www.agentty.run/docs/launch)** — 把几句话变成智能体能构建的项目，然后部署到 GitHub、Vercel 和 Supabase。
- **[Claude 顾问](https://www.agentty.run/docs/advisor)** — 让 Claude Code 在关键时刻咨询更强大的模型，可按标签页配置。
- **文件和编辑器** — 项目树和编辑器，支持语法高亮和格式化工具，加上"在编辑器中打开"支持 VS Code 或 Cursor。
- **命令面板、跳到未读和迷你模式** — ⇧⌘P、⇧⌘U 和 ⌃⌘M；参见[快捷键](#快捷键)。

---

## 支持的智能体

适用于**任何 CLI 智能体** — 如果它能在终端中运行，就能在 Agentty 中运行。Agentty 会检测这些并在启动器中提供：

<p>
  <a href="https://docs.anthropic.com/en/docs/claude-code"><kbd><img src="https://www.google.com/s2/favicons?domain=claude.ai&amp;sz=64" alt="" width="16" valign="middle" /> Claude Code</kbd></a> &nbsp;
  <a href="https://github.com/openai/codex"><kbd><img src="https://www.google.com/s2/favicons?domain=openai.com&amp;sz=64" alt="" width="16" valign="middle" /> Codex</kbd></a> &nbsp;
  <a href="https://github.com/google-gemini/gemini-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=gemini.google.com&amp;sz=64" alt="" width="16" valign="middle" /> Gemini CLI</kbd></a> &nbsp;
  <a href="https://antigravity.google"><kbd><img src="https://www.google.com/s2/favicons?domain=antigravity.google&amp;sz=64" alt="" width="16" valign="middle" /> Antigravity CLI</kbd></a> &nbsp;
  <a href="https://ampcode.com"><kbd><img src="https://www.google.com/s2/favicons?domain=ampcode.com&amp;sz=64" alt="" width="16" valign="middle" /> Amp</kbd></a> &nbsp;
  <a href="https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=github.com&amp;sz=64" alt="" width="16" valign="middle" /> GitHub Copilot CLI</kbd></a> &nbsp;
  <a href="https://cursor.com/cli"><kbd><img src="https://www.google.com/s2/favicons?domain=cursor.com&amp;sz=64" alt="" width="16" valign="middle" /> Cursor CLI</kbd></a> &nbsp;
  <a href="https://opencode.ai"><kbd><img src="https://www.google.com/s2/favicons?domain=opencode.ai&amp;sz=64" alt="" width="16" valign="middle" /> OpenCode</kbd></a> &nbsp;
  <a href="https://github.com/QwenLM/qwen-code"><kbd><img src="https://www.google.com/s2/favicons?domain=qwenlm.github.io&amp;sz=64" alt="" width="16" valign="middle" /> Qwen Code</kbd></a> &nbsp;
  <a href="https://docs.factory.ai/cli/getting-started/quickstart"><kbd><img src="https://www.google.com/s2/favicons?domain=factory.ai&amp;sz=64" alt="" width="16" valign="middle" /> Factory Droid</kbd></a> &nbsp;
  <a href="https://block.github.io/goose/"><kbd><img src="https://www.google.com/s2/favicons?domain=goose-docs.ai&amp;sz=64" alt="" width="16" valign="middle" /> Goose</kbd></a> &nbsp;
  <a href="https://github.com/charmbracelet/crush"><kbd><img src="https://www.google.com/s2/favicons?domain=charm.sh&amp;sz=64" alt="" width="16" valign="middle" /> Crush</kbd></a> &nbsp;
  <a href="https://aider.chat"><kbd><img src="https://aider.chat/assets/icons/favicon-32x32.png" alt="" width="16" valign="middle" /> Aider</kbd></a> &nbsp;
  <a href="https://github.com/MoonshotAI/kimi-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=moonshot.cn&amp;sz=64" alt="" width="16" valign="middle" /> Kimi CLI</kbd></a> &nbsp;
  <a href="https://kiro.dev/docs/cli/"><kbd><img src="https://www.google.com/s2/favicons?domain=kiro.dev&amp;sz=64" alt="" width="16" valign="middle" /> Kiro CLI</kbd></a> &nbsp;
  <a href="https://docs.cline.bot/cline-cli/overview"><kbd><img src="https://www.google.com/s2/favicons?domain=cline.bot&amp;sz=64" alt="" width="16" valign="middle" /> Cline CLI</kbd></a> &nbsp;
  <a href="https://x.ai/cli"><kbd><img src="https://www.google.com/s2/favicons?domain=x.ai&amp;sz=64" alt="" width="16" valign="middle" /> Grok Build</kbd></a> &nbsp;
  <a href="https://ollama.com"><kbd><img src="https://www.google.com/s2/favicons?domain=ollama.com&amp;sz=64" alt="" width="16" valign="middle" /> Ollama (本地模型)</kbd></a> &nbsp;
  <kbd>+ 任何 CLI 智能体</kbd>
</p>

Claude Code 和 Codex 获得一级面板：来自它们的钩子的实时状态、会话、用量和成本，以及两者之间的交接。

---

## 安装

### macOS — Apple 芯片，macOS 13+

```sh
brew install --cask empty-user77/agentty/agentty
```

就这样 — Agentty 已在你的应用程序文件夹中。还没有 Homebrew？先用 [brew.sh](https://brew.sh) 上的一行命令安装。
Agentty 自动更新；`brew upgrade --cask agentty` 也可用，`brew uninstall --cask agentty` 可卸载。

喜欢下载？从[最新版本](https://github.com/empty-user77/Agentty/releases/latest)获取 `Agentty-X.Y.Z-…-arm64.dmg`，打开后把 **Agentty** 拖进
**应用程序**。它由 Apple 签名和公证。没有 Intel Mac 版本。

### Windows — 10 版本 1809+，x64

1. 从[最新版本](https://github.com/empty-user77/Agentty/releases/latest)下载 `Agentty-X.Y.Z-windows-x64-setup.exe`。如果浏览器拦截 `.exe`，取
   `Agentty-X.Y.Z-windows-x64-setup.zip` 并解压：里面是同一个安装程序。
2. 运行它。它只为你的用户安装且不需要管理员权限。
3. 安装程序尚未进行代码签名。如果出现 SmartScreen，选择**更多信息 → 仍要运行**。

卸载：设置 → 应用 → Agentty → 卸载。

### Linux — x86_64

从[最新版本](https://github.com/empty-user77/Agentty/releases/latest)下载适合你发行版的包，然后：

```sh
# Debian 12+ / Ubuntu 22.04+
sudo apt install ./Agentty-*-linux-amd64.deb

# RHEL 9+ / Fedora
sudo dnf install ./Agentty-*-linux-x86_64.rpm
```

卸载：`sudo apt remove agentty` 或 `sudo dnf remove agentty`。

### 更新

Agentty 在启动时和每小时检查新版本。在 macOS 和 Windows 上，它下载更新、对照发布校验和验证、安装并重启。在 Linux 上，
它告诉你有新版本并链接到包。

### 开始前

Agentty 运行你的智能体，但不自带它们。安装你想使用的 CLI，例如
[Claude Code](https://docs.anthropic.com/en/docs/claude-code)（`claude`）和 [Codex](https://github.com/openai/codex)
（`codex`），并确保它们在你的 `PATH` 中。Agentty 的设置 → 系统可以安装缺少的部分。

### 从源代码构建

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

你需要 Rust 1.98（固定在 `rust-toolchain.toml` 中）和你平台的构建工具；Windows 和 Linux 请参见
[docs/platforms.md](docs/platforms.md)。

---

## 快捷键

| 操作 | 快捷键 |
|---|---|
| 新建终端 / Claude Code / Codex 标签页 | ⌘T / ⌥⌘C / ⌥⌘X |
| 向右 / 向下分屏 | ⌘D / ⇧⌘D |
| 下一个 / 上一个 分屏 · 标签页 · 工作区 | ⌘] ⌘[ · ⇧⌘] ⇧⌘[ · ⌥⌘↓ ⌥⌘↑ |
| 命令面板 · 跳到未读 | ⇧⌘P · ⇧⌘U |
| Git · Session Flow · 监控 · 设置 | ⇧⌘G · ⇧⌘F · ⌥⌘U · ⌘, |
| 文件面板 · 应用内浏览器 · 迷你模式 | ⌥⌘B · ⇧⌘B · ⌃⌘M |

完整列表在 **设置 → 键盘快捷键**。在 Windows 和 Linux 上，⌘ 变为 Ctrl+Shift，⇧⌘ 变为
Ctrl+Alt+Shift，⌥⌘ 变为 Ctrl+Alt，这样 Ctrl+字母 还是给 shell。

## 插件

插件填充终端旁边的面板，在智能体分屏上方添加按钮和命令面板条目，并可以发送提示词 — 发送到哪里始终由你在"发送到…"对话框中选择。插件页面在安装前显示它申请的权限；这些限制了它能请求 Agentty 做的事。WebAssembly 插件无法接触这些权限以外的任何东西。以 Node.js、Python 或原生程序运行的插件在 Agentty 外拥有和你相同的文件和网络访问权限，所以只安装你信任的。插件通过
[Agentty Marketplace](https://github.com/empty-user77/Agentty-Marketplace) 发布。

内置的 **Cosmica** 插件把 [Cosmica](https://www.cosmica.ink/) 笔记变成提示词并把会话摘要写回。要编写自己的，参见 [docs/plugins](docs/plugins/README.md)；插件页面也能创建一个并让 Claude Code 构建。

## 文档

| | |
|---|---|
| [agentty.run/docs](https://www.agentty.run/docs) | 应用各部分的说明 |
| [docs/zh](docs/zh/introduction.md) | 同样的用户文档作为 Markdown ([English](docs/en/introduction.md) · [한국어](docs/ko/introduction.md) · [日本語](docs/ja/introduction.md)) |
| [docs/configuration.md](docs/configuration.md) | `settings.json`、智能体登录、harness、数据库、主题和 Agentty 保留的文件 |
| [docs/platforms.md](docs/platforms.md) | Windows 和 Linux 的差异以及如何在那里构建 |
| [docs/plugins](docs/plugins/README.md) | 编写插件（Node.js SDK 或[协议](docs/plugins/protocol.md)）和[使用方法](docs/plugins/usage.md) |
| [docs/architecture.md](docs/architecture.md) | crate、终端、智能体套接字和面板如何组合 |
| [docs/metrics.md](docs/metrics.md) | 官方构建可能发送的每个事件以及如何关闭 |
| [docs/release.md](docs/release.md) | 打包、签名、公证和发布渠道 |

---

## 社区和支持

- **X：** 关注 **[@agentty_run](https://x.com/agentty_run)** 以获取更新和公告。
- **反馈和想法：** 缺少什么或发现了错误？[开启一个问题](https://github.com/empty-user77/Agentty/issues)。
- **安全：** 按 [SECURITY.md](SECURITY.md) 中的描述私下报告漏洞。
- **贡献：** 阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 和[行为准则](CODE_OF_CONDUCT.md)。
- **隐私：** 智能体的文件记录**仅在本地**被读取以列出会话、计算用量和构建交接；提示词、输出、路径和仓库数据永远不会上传。官方发布的构建发送匿名使用统计（使用了哪些功能、应用和系统版本以及随机安装 ID）。在设置 → 通用中关闭或设置 `DO_NOT_TRACK=1`；从源代码构建不发送任何东西。每个事件都列在 [docs/metrics.md](docs/metrics.md) 中。
- **表示支持：** [Star](https://github.com/empty-user77/Agentty) 这个仓库以跟进。

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

完整文本见 [LICENSE](LICENSE)。捆绑字体和第三方组件保留各自的许可证；参见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
