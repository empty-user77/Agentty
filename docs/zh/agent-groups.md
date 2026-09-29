---
title: 智能体组
description: 在一个项目中一起启动一组各有角色的智能体。
---

智能体组会在项目的一个标签页中打开多个智能体，每个都有自己的角色：一个开发，一个审查，一个部署。
每个智能体在终端上方带着名字和角色徽章，知道其他成员的角色，并且从一开始就彼此相连，一个做了什么，
其他成员也知道。

## 启动智能体组

在项目中打开命令面板（**⇧⌘P**），选择 **启动智能体组：…**。只会列出此项目可用的组。第一个智能体
是主智能体：它的窗格以放大状态启动，其他成员在旁边工作。

在 Claude Code 尚未信任的文件夹中，只有主智能体会先启动。在主窗格中回答文件夹信任问题后，其余成员
会启动，不再重复询问。

## 智能体如何协作

- **每个请求都按该智能体的角色理解。** 对部署智能体说"部署 e-ticket"，就是按此项目的部署方式完成
  e-ticket 的部署工作。
- **属于其他智能体角色的工作会交给它。** 智能体运行 `agentty group send "<智能体>" "<请求>"`，
  请求会进入该智能体的终端（如果它正在工作，则在当前回合结束后），你可以在那里跟进。
- **不属于任何角色的工作不会执行。** 智能体会告诉你组里有哪些角色。
- **共享会话。** Claude Code 智能体会被互相介绍，并通过 Claude Code 自己的会话消息直接交流；其他
  智能体会获得双向的实时 [Session Flow](session-flow) 连接。

`agentty group list` 显示成员、角色以及是否正在工作。

## 智能体组就是一个包

一个组就是一个 JSON 文件。分享这个文件，收到的人在命令面板中选择 **导入智能体组…** 即可立即启动。
已安装的组位于 `~/.agentty/agent-groups/`（面板中的 **打开智能体组文件夹**）；在那里编辑文件即可修改
组，复制一个即可新建。其中有一个示例组 *Dev team* 可作为起点。

```json
{
  "name": "E-ticket team",
  "description": "Build and ship the e-ticket service",
  "scope": "acme/eticket",
  "notes": "Deploy with ./scripts/deploy.sh <env>. Staging first.",
  "agents": [
    { "name": "Dev", "role": "Implements features and fixes bugs", "badge": "Build", "agent": "claude" },
    { "name": "Deploy", "role": "Builds and deploys to staging and production", "badge": "Release",
      "agent": "claude", "instructions": "Never deploy to production without the user's OK." }
  ]
}
```

| 字段 | 含义 |
|---|---|
| `name` | 组名，显示在命令面板中。 |
| `description` | 一行组说明。 |
| `scope` | 所有项目可用为 `"any"`；只用于某个仓库为 `"owner/repo"`：仅在 `origin` 远程为该仓库的项目中显示。 |
| `notes` | 每个智能体都应知道的内容：如何构建和部署、文件在哪里。 |
| `agents` | 1 到 6 个智能体；第一个是主智能体。 |
| `agents[].name` | 显示在窗格上，也用于移交工作。名字不能重复。 |
| `agents[].role` | 一行说明此智能体做什么。 |
| `agents[].badge` | 角色徽章上的一两个词（最多 16 个字符）；缺省时取 `role` 的开头。 |
| `agents[].agent` | `claude`（默认）或 `codex`。 |
| `agents[].instructions` | 此智能体的额外规则。 |

组的角色、说明和指示会成为给智能体的指令。导入别人分享的组之前，请像读脚本一样读一遍：它决定了
智能体在你的项目中做什么。

## 须知

- 组徽章和连接只在应用运行期间保留；重启后智能体的标签页会恢复，但没有徽章和连接。需要新团队时请
  重新启动该组。
- 关闭某个智能体的窗格后，它就退出了；向它移交工作时会提示它已关闭。
