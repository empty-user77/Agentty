---
title: 常见问题
description: 使用 Agentty 过程中常见问题的简短解答。
---

## Agentty 免费吗？

免费。没有账号、没有订阅、没有付费档位。智能体消耗的费用由你直接支付给各 AI 提供方，Agentty 不参与其中。

## 它和 tmux 有什么不同？

tmux 负责复用终端。Agentty 是围绕在其中运行的智能体构建的：它知道哪个在工作、哪个在等你、各自改了什么、花了多少钱。标签页、分屏和真正的终端你照样拥有 —— 再加上那些只有当窗格里跑的是 AI 智能体时才有意义的东西。

## 我的对话会被上传吗？

不会。会话列表、搜索、用量数字和上下文文档，全都由你电脑上已有的文件生成。你运行的智能体 CLI 直接与各自的提供方通信 —— Claude Code 连 Anthropic，Codex 连 OpenAI —— 和在任何其他终端里一样，受那些提供方的条款约束。

Agentty 自身发送的内容列在[使用统计](/docs/telemetry)中，完整说明见[隐私政策](https://www.agentty.run/privacy-policy)。

## + 菜单里没有我的智能体

Agentty 只提供能在你**登录 shell** 的 `PATH` 中找到的 CLI。如果在终端里能用、这里却没有，多半是 `PATH` 写在了只有交互式 shell 才读取的文件里。**设置 → 系统检查**会列出缺失项，并可以帮你安装。

## 怎么重新打开旧的对话？

**⇧⌘S** 会列出你机器上所有 Claude Code 与 Codex 会话，**⇧⌘O** 可按标题、内容或路径搜索。会话是智能体自己的文件，Agentty 只是读取它们。参见[会话](/docs/sessions)。

## 分支和工作树越积越多

每个并行会话都会得到一个 `agentty/<名称>` 分支上的工作树。[文件面板](/docs/agent-git)（**⌥⌘B**）列出所有工作树；右键即可移除，合并之后连分支一起删除。

如果你更想在一个文件夹里工作，请关闭**设置 → 常规 → 每个会话独立工作树**。

## 用量数字和我的账单对不上

它们是根据本地会话记录计算的估算值，而且只有 Claude 的价格是内置的。其他模型请把价格加到 `~/.agentty/pricing.json` —— 参见[配置参考](/docs/configuration-reference)。真正的数字以提供方的账单为准。

## 智能体一直请求权限

那是智能体自身的权限机制，不是 Agentty 的。请在智能体那边配置 —— Claude Code 的话是它自己的设置或 `/permissions`。Agentty 只是原样传递你的选择，不会替你放宽。

## 插件加载不了

把运行所需的一切都打包进去，包括 SDK 和任何 `node_modules` —— Agentty 不会执行 `npm install`。文件夹名必须与清单中的 `id` 一致。插件卡片上的**日志**会显示真正的错误。参见[发布插件](/docs/plugin-publishing)。

## Windows 或 Linux 版本什么时候能用？

代码在两个平台上都能构建和运行，安装包也在计划中。目前唯一公开下载的是 macOS 版。参见[平台](/docs/platforms)。

## 源码公开了吗？

还没有，开源在计划中。在此之前，发布页面就是获取 Agentty 的方式。

## 还有别的问题

[故障排查](/docs/troubleshooting)覆盖了常见情形。其他问题可以到[问题跟踪](https://github.com/empty-user77/Agentty/issues)反馈；安全问题请私下报告，不要开公开 issue。
