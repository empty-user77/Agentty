---
title: 扩展、MCP 与连接器
description: 技能、子智能体、命令、MCP 服务器，以及你自己的 HTTP API —— 集中在一处，密钥交给操作系统保管。
---

**⇧⌘X** 打开扩展：Claude Code 与 Codex 的技能、子智能体、命令、插件和 MCP 服务器集中在一个列表中，并显示每台服务器的实时连接状态。

在这里可以一键添加官方 MCP 服务器，或把其中任何一个插入并运行在当前活动的智能体中。智能体窗格的状态栏会显示该智能体实际拥有的技能、子智能体和 MCP 服务器。

## MCP 服务器

Agentty 不做中转，而是把服务器注册给智能体 CLI：服务器被加入项目或智能体自身的配置，智能体直接与它通信。

当服务器 URL 或参数中的某个值看起来像密钥时，显示时会被遮蔽。

## API 连接器

连接器把任意 HTTP API 变成智能体可用的 MCP 服务器。你只需描述一次端点 —— 方法、路径、各参数的含义 —— 智能体就获得了可以调用的工具。

- 定义存放在 `~/.agentty/connectors.json`。
- **密钥不会进入该文件。** 它们保存在操作系统的凭据存储中：macOS 是钥匙串，Windows 是凭据管理器，Linux 是 Secret Service；若都不存在，则使用仅你的账号可读的私有文件。
- 可能含密钥的值，在任何显示位置都会被遮蔽。

## 智能体登录

**设置 → 账号**决定新的智能体标签页如何登录，面向那些无法使用 CLI 自身登录的机器：

| 智能体 | 方式 |
|---|---|
| Claude Code | API 密钥、网关令牌、`claude setup-token` 的 OAuth 令牌、Amazon Bedrock、Google Vertex AI |
| Codex | API 密钥，或导入的 `auth.json` |

方式与非密钥值保存在 `~/.agentty/agent-auth.json`，密钥和令牌进入凭据存储。**测试**会用提供方自己的端点验证密钥。

凭据只到达智能体进程。智能体退出后留下的 shell 不会保留它们，属于其他登录方式的变量会在智能体环境中被移除，因此你在 shell 配置里 export 的密钥无法覆盖你选定的方式。

如果保存的方式无法使用（例如密钥已从凭据存储中删除），窗格会说明情况，智能体回退到自身的登录。

## 智能体 harness

如果项目声明了 harness，进入它时会出现**以 harness 开始**：选一个入口（例如 `/implement`），粘贴工单号或链接，智能体就以此开始。

满足下列任一条件即视为 harness：`.harness` 文件、`harness.json` / `.yaml` / `HARNESS.md`、名为 `harness` 或 `harness-*` 的项目技能、`agentty.json` 中的 `harness` 列表，以及你在**设置 → 项目**中添加的模式。

项目可以自定义入口：

```json
{
  "harness": [
    { "label": "Start a ticket", "command": "/implement", "input": "Ticket key or URL" },
    { "label": "Fix a bug", "prompt": "Reproduce and fix {input}", "agent": "codex" }
  ]
}
```

`command` 会把输入追加在后面；在 `prompt` 中，`{input}` 会被替换为输入。
