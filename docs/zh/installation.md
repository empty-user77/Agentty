---
title: 安装
description: 获取适用于 macOS、Windows 和 Linux 的 Agentty，并准备好配套的智能体 CLI。
---

Agentty 免费且无需账号。下载、拖入、打开终端即可。

## 下载

所有版本都发布在[发布页面](https://github.com/empty-user77/Agentty/releases)。

**macOS 13 及以上，Apple 芯片** —— 打开 `Agentty-X.Y.Z-…-arm64.dmg`，把 **Agentty** 拖进**应用程序**。应用已用 Developer ID 签名并通过 Apple 公证，因此不会出现警告。

> [!NOTE]
> 没有 Intel Mac 版本。macOS 版 Agentty 仅支持 Apple 芯片。

**Windows 10 1809 及以上，x64** —— 运行 `Agentty-X.Y.Z-windows-x64-setup.exe`。按用户安装，无需管理员权限。

> [!IMPORTANT]
> **Windows 安装程序尚未进行代码签名**，因此 Windows SmartScreen 可能提示发布者未知。要继续，请选择**更多信息 → 仍要运行**。如果浏览器直接拒绝下载 `.exe`（Chrome 会拦截未签名的可执行文件），请改为下载 `Agentty-X.Y.Z-windows-x64-setup.zip` 并解压，里面是同一个安装程序。

**Linux，x86_64** —— Debian 12+ / Ubuntu 22.04+：`sudo apt install ./Agentty-X.Y.Z-linux-amd64.deb`。RHEL 9+ / Fedora：`sudo dnf install ./Agentty-X.Y.Z-linux-x86_64.rpm`。

所有文件的校验和见 `Agentty-X.Y.Z-SHA256SUMS.txt`。三个平台的差异请参见[平台](/docs/platforms)。

## 智能体命令行工具

Agentty 负责运行智能体，但不会捆绑它们。请自行安装所需工具，并确保它们在登录 shell 的 `PATH` 中可被找到。

- **Claude Code** —— `claude`
- **Codex** —— `codex`
- 可选：Gemini CLI、Copilot CLI、Cursor CLI、Grok、OpenCode、Qwen Code、Amp、Droid、Goose、Crush、Aider，以及本地的 Ollama 模型

Agentty 只检测并提供你已安装的工具。没装的不会先出现再失败，而是根本不显示。

> [!TIP]
> 如果某个工具在终端里能用、在 Agentty 里却不行，通常是因为 `PATH` 被写在只有交互式 shell 才读取的文件里。Agentty 通过登录 shell 启动窗格，请把 `PATH` 写在登录 shell 会读取的位置。

你也可以不使用 CLI 自带的登录方式，在**设置 → 账号**中用 API 密钥、网关令牌、`claude setup-token`、Amazon Bedrock、Google Vertex AI，或导入的 Codex `auth.json` 来启动。

### Node.js

用 JavaScript 编写的插件需要 `PATH` 中有 **Node.js 18 或更高版本**。Agentty 本身不需要。

### Git

git 页面、工作树和分支切换使用 `PATH` 中的 `git`。在 Windows 上，Claude Code 的钩子需要 **Git for Windows**。

## 更新

Agentty 在启动时和每小时检查一次新版本。

提示中的**稍后**会推迟一天，而不是推迟到下次检查；这样等待中的更新会一直显示在状态栏里，推迟不等于忘记。

- **macOS 和 Windows** —— 下载更新，用发布校验和验证后安装并重启。
- **Linux** —— 只提示有新版本并给出发布页面链接，新的 `.deb` 或 `.rpm` 请用包管理器自行安装。

## 从源码构建

源码仓库尚未公开。我们计划将其开源；在此之前，发布页面上的构建就是运行 Agentty 的方式。

## Agentty 的文件位置

| 路径 | 用途 |
|---|---|
| `~/.agentty/settings.json` | 偏好设置 |
| `~/.agentty/workspaces.json` | 工作区、标签页、分屏和分组 |
| `~/.agentty/plugins/` | 已安装的插件 |
| `~/.agentty/handoffs/` | Session Flow 与对话迁移生成的上下文文档 |
| `~/.agentty/connectors.json` | API 连接器定义（密钥存放在系统凭据存储中） |

完整清单见[配置参考](/docs/configuration-reference)。

## 卸载

从应用程序文件夹删除 `Agentty.app` 即可。

若要一并清除数据，请删除 `~/.agentty`。智能体 CLI 写入的会话位于它们各自的文件夹（`~/.claude`、`~/.codex`），不归 Agentty 清理。

## 接下来

- [快速开始](/docs/quick-start) —— 最初的十分钟
- [平台](/docs/platforms) —— Windows 与 Linux 上的差异