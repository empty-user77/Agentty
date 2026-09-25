---
title: Claude Advisor
description: 让 Claude Code 在关键时刻咨询更强的模型，可在 Agentty 中按标签页设定。
---

Claude Code 可以在一轮工作的关键节点向更强的模型请教 —— 大规模重构之前、计划需要判断时、卡住的时候。Agentty 把它做成一个设置项，你不必记住命令行参数。

> [!NOTE]
> 这是 Claude Code 的实验功能，会额外消耗 token。除非你主动打开，否则不会启用。

## 工作原理

执行咨询的不是 Agentty。Agentty 在启动 Claude Code 窗格时向命令行追加 `--advisor <模型>`，真正的咨询由 Claude Code 完成。因此：

- 顾问使用的是**你自己的** Claude Code 登录和提供方账号，和会话其余部分一样。
- 不会写入任何配置文件。选择保存在 Agentty 的设置里，每次启动时传递。
- 选择**关闭**会传入一个开关，覆盖你可能在 Claude Code 里设过的 `advisorModel`，所以那个窗格是真正关闭的。

## 选择模型

**设置 → 常规 → Claude Advisor** 作用于**新打开的** Claude 标签页：

| 选项 | 效果 |
|---|---|
| Claude Code 设置 | Agentty 什么都不传，按 Claude Code 自身配置执行 |
| 关闭 | 该窗格禁用顾问，优先于 Claude Code 自身设置 |
| Opus | 咨询 Opus |
| Fable | 咨询 Fable |

已经在运行的窗格可以从状态栏更改，无需重启会话。

## 什么时候值得开

- **值得：** 架构决策、涉及多个文件的重构、前三个猜想都落空的调试、智能体确定方案前的评审。
- **不值得：** 日常编辑、测试、格式化等机械性工作。额外的 token 买不到任何东西。

一个稳妥的默认做法是平时关着，等某个会话走到你在意的决策点时再从状态栏打开。
