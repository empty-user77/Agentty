---
title: 配置参考
description: Agentty 保存的每个文件，以及 settings.json 中的每个键。
---

Agentty 把所有东西都放在 `~/.agentty/`。其中大部分在**设置**（⌘,）里改更方便。

## 文件

| 路径 | 用途 |
|---|---|
| `~/.agentty/settings.json` | 偏好设置 |
| `~/.agentty/workspaces.json` | 工作区、标签页、分屏和分组 |
| `~/.agentty/themes/*.itermcolors` | 导入的配色主题 |
| `~/.agentty/pricing.json` | 非 Claude 模型的价格 |
| `~/.agentty/handoffs/` | Session Flow 与智能体迁移产生的上下文文档 |
| `~/.agentty/connectors.json` | API 连接器定义（不含密钥） |
| `~/.agentty/agent-auth.json` | 新智能体标签页的登录方式（不含密钥） |
| `~/.agentty/codex-home/` | 用 API 密钥登录时的专用 Codex 主目录 |
| `~/.agentty/worktrees/` | 按会话创建的工作树 |
| `~/.agentty/plugins/` | 已安装的插件 |
| `~/.agentty/plugin-data/<id>/` | 各插件自己的数据 |
| `~/.agentty/prompts/` | 以文件形式交给智能体的长提示词 |
| `~/.agentty/install_id` | 用于使用统计的随机安装标识符 |
| `agentty.json`（项目）或 `~/.agentty/commands.json` | 命令面板的自定义条目 |

密钥不会出现在这些文件中，它们存放在操作系统的凭据存储里。

## settings.json

```json
{
  "language": "zh",
  "theme": "Agentty Dark",
  "fontFamily": "JetBrains Mono",
  "fontSize": 13.0,
  "lineHeight": 1.25,
  "cursorShape": "block",
  "scrollback": 10000,
  "askDirectory": true,
  "autoWorktree": true,
  "agentTasks": true,
  "analytics": true,
  "agentBarPosition": "top"
}
```

| 键 | 取值 |
|---|---|
| `language` | `en`、`ko`、`ja`、`zh` |
| `theme` | 内置主题名，或导入的 `.itermcolors` 去掉扩展名的文件名 |
| `fontFamily`、`fontSize`、`lineHeight`、`padding` | 外观 |
| `cursorShape` | `block`、`beam`、`underline`；`cursorBlink` 控制闪烁 |
| `letterSpacing` | 每个终端单元格额外增加的宽度（点，0–8） |
| `boldText` | 以粗体绘制普通终端文字；原本加粗的部分会更粗 |
| `colorBackground`、`colorForeground`、`colorCursor`、`colorSelection` | 手动替换的主题颜色，以 `0xRRGGBB` 数值书写 —— JSON 没有十六进制字面量，所以 `0x121212` 写作 `1184274`。不设置即为主题自带的颜色 |
| `scrollback` | 每个终端保留的行数 |
| `askDirectory` | 新建工作区时询问文件夹；`askDirectoryForTabs` 对标签页同理 |
| `resumeBar` | 终端进入含有早先会话的文件夹时提示恢复 |
| `autoWorktree` | 繁忙项目中的第二个会话获得独立工作树 |
| `agentTasks` | 允许智能体请求开始并行任务，每次都要确认 |
| `agentGuide` | 从这里启动的智能体会收到 Agentty 命令的简短说明 |
| `stopServersOnClose` | 关闭窗格时停止其中启动的服务器 |
| `confirmClose` | 关闭用过的东西前先确认 |
| `agentBarPosition` | `top` 或 `bottom` |
| `workspaceSearchBar` | 工作区列表上方的搜索框，搜索名称及其中的对话（默认开启） |
| `sortFinishedToTop` | 智能体完成后把该工作区移到顶部（默认关闭；关闭时保留你自行排列的顺序） |
| `hud` | 状态栏项目及顺序：`model`、`context`、`usage`、`status`、`elapsed`、`links`、`spacer`、`ports`、`worktree`、`branch`、`folder` |
| `browser.autoOpenServers` | 开发服务器有响应后在内置浏览器中打开 |
| `linkOpener` | ⌘-点击使用内置浏览器还是默认浏览器 |
| `externalEditor` | `auto`、`vsCode`、`cursor`、`system` |
| `harnessDetect`、`harnessPatterns`、`harnessSubmit`、`harnessAgent` | harness 的检测与行为 |
| `systemNotifications`、`notifyWhenFocused`、`notifyAnswerRequests`、`chatNotify` | 通知 |
| `chatNotify.slackBot`、`chatNotify.discordBot` | 用机器人令牌和频道发送，而不是 Webhook URL |
| `chatNotify.slackChannel`、`chatNotify.discordChannel` | 机器人写入的频道 —— Slack 可用 `#general`、`general` 或频道 id；Discord 用频道 id |
| `analytics` | 是否同意匿名使用统计。`false` 或 `DO_NOT_TRACK=1` 时不发送任何内容 |
| `menuBar` | 菜单栏图标（macOS） |

## 自定义命令

项目中的 `agentty.json`，或对所有项目生效的 `~/.agentty/commands.json`，可向命令面板添加条目：

```json
{
  "commands": [
    { "label": "Run tests", "command": "npm test" },
    { "label": "Deploy staging", "command": "./scripts/deploy.sh staging" }
  ]
}
```

## pricing.json

为价格未内置的模型指定每百万令牌的美元价格：

```json
{
  "some-model": { "input": 1.25, "output": 10.0, "cacheRead": 0.125 }
}
```

`cacheRead`、`cacheWrite` 和 `cacheWrite1h` 为可选，默认分别是 `input` 的 10%、125% 和 200%。
