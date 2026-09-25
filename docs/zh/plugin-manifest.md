---
title: 清单参考
description: agentty-plugin.json 的所有字段 —— 标识信息、运行方式、权限、面板、命令与图标。
---

`agentty-plugin.json` 位于插件文件夹的根目录，告诉 Agentty 这个插件是什么、如何启动、能做什么，以及为界面添加了什么。

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "runtime": "node",
  "apiVersion": 1,
  "description": "Turns the current folder into a prompt.",
  "publisher": "you",
  "homepage": "https://example.com/hello",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [
      { "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }
    ]
  }
}
```

## 标识信息

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 由 `a-z`、`0-9`、`-` 组成的 2–40 个字符。**必须与文件夹名相同。** |
| `name` | 是 | 显示在商店和面板按钮上 |
| `version` | 是 | `major.minor.patch` |
| `description` | | 商店卡片上的一行说明 |
| `publisher` | | 作者 |
| `homepage` | | 必须是 `https://` |
| `keywords` | | 用于商店搜索 |
| `links` | | 最多 6 个 `{ "label", "url" }`，作为按钮显示在卡片上（项目主页、文档、源码） |
| `icon` | | 下方列表中的图标名 |
| `logo` | | 插件文件夹内的图片文件，代替 `icon` 绘制。模块则把徽标放在模块里 —— 见[徽标](#徽标) |

## 运行

| 字段 | 默认值 | 说明 |
|---|---|---|
| `main` | 必填 | 相对于插件文件夹的入口点 |
| `runtime` | `node` | `node`（登录 shell `PATH` 中的 Node.js 18+）、`python`（`python3 <main>`）、`executable`（直接运行 `<main>`），或 `wasm` —— `<main>` 是 Agentty 自己运行的 WebAssembly 模块，见 [Rust 与 WebAssembly](/docs/plugin-rust) |
| `apiVersion` | `1` | 编写时依据的插件 API 版本。`2` 增加了 [AgentOS 插件](/docs/plugin-agentos)所需的 `host/timer` 和 `pane/status`。只懂旧版本的 Agentty 会直说，而不是安装一个自己跑不了的东西 |
| `activationEvents` | `[]` | `["onStartup"]` 表示随 Agentty 一起启动，否则在首次使用时启动 |

Agentty 以插件文件夹作为工作目录启动程序。`wasm` 插件不启动任何程序：模块在 Agentty 内部运行，没有工作目录、没有环境变量、也没有文件。

## 与其他应用的集成

| 字段 | 说明 |
|---|---|
| `requires` | `{ "name", "url", "note" }` —— 该插件面向的应用或服务。卡片会显示是否检测到，未检测到时提供链接 |
| `detect` | 集成目标应用的路径（可用 `~`）。检测到则卡片标记为**推荐** |

## 权限

```json
"permissions": ["net.request", "prompt.inject", "terminal.write", "session.read", "workspace.read"]
```

| 权限 | 允许的事 |
|---|---|
| `net.request` | 向插件指定的地址发送 HTTP 请求 |
| `prompt.inject` | 发送提示词 |
| `terminal.write` | 向已打开的窗格输入 |
| `session.read` | 读取 AI 对话 |
| `workspace.read` | 列出工作区，查看上下文中的文件夹与标题字段 |

只申请你会用到的。清单会在安装前展示给用户，没有权限的调用会失败。参见[插件权限](/docs/plugin-permissions)。

## 插件添加的内容

### 面板

```json
"contributes": { "panel": { "title": "Hello", "icon": "sparkles", "surface": "sidebar", "mode": "push" } }
```

给插件一个按钮，以及一块由它用 UI 树填充的面板（宽 360 px，纵向滚动）。见 [SDK](/docs/plugin-sdk)。

`surface` 决定按钮放在哪里：

| `surface` | 位置 |
|---|---|
| `pane`（默认） | 终端上方的标签栏 |
| `sidebar` | 左侧边缘的活动栏，与 Agentty 自己的页面并列 |
| `status` | 底部的状态栏 |

`mode` 决定面板怎么打开。用户可以用面板上的布局按钮更改，并且选择会被记住；这里说的是它一开始的行为：

| `mode` | |
|---|---|
| `push`（默认） | 停靠在终端旁边，终端让出位置 |
| `overlay` | 浮在窗口右侧边缘，其他东西不动 |
| `window` | 独立窗口，可移动、可调整大小 |
| `full` | 终端和页面所占的整个区域 |

停靠的面板不会大到把窗口其余部分挤扁：拖过可停靠的范围，它就变成浮动面板。

### 命令

```json
"contributes": {
  "commands": [
    {
      "id": "hello.explain",
      "title": "Hello: Explain this folder",
      "description": "Sends a tour request to the focused agent",
      "icon": "bot",
      "palette": true
    }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `id` | 插件内唯一；SDK 以此 id 注册处理器 |
| `title` | 显示在命令面板中，前面加上插件名便于归类 |
| `description` | 可选的第二行 |
| `icon` | 在命令面板中显示在命令旁边的图标名 |
| `palette` | 为 `false` 时从命令面板中隐藏 |

命令从命令面板中调用。`paneBar` 和 `when` 已经取消：仍带着这两个字段的清单照常安装和运行，字段会被忽略；但**依赖这些按钮构建的插件会失去按钮**，其命令改为从命令面板中调用。

## 图标

`icon` 字段请使用下列名称，其他值会显示为拼图块。

```
app-window arrow-down arrow-left arrow-right arrow-up arrow-up-right at-sign bell bell-dot
blocks book-open bookmark bot brain bug calendar chart-column check chevron-down chevron-right
chevron-up circle-check circle-dot circle-pause circle-x clipboard clipboard-paste clock cloud
code columns-2 command container copy database download ellipsis external-link eye file-input
file-plus file-text folder folder-open folder-plus git-branch git-commit-horizontal
git-pull-request globe grip-vertical hammer hash history house image info key-round
layout-panel-left lightbulb link list list-tree loader-circle mail maximize-2 message-circle-question
message-square minimize-2 minus network notebook notebook-pen package panel-left-close
panel-left-open pencil picture-in-picture-2 play plug plus power puzzle refresh-cw rocket rotate-cw
rows-2 save scroll-text search send settings shield-alert sparkles square square-plus
square-terminal star sticky-note tag terminal trash-2 undo-2 unlink upload users wand-sparkles
workflow wrench x zap git-fork file lock graduation-cap x-twitter
```

## 徽标

徽标是插件自己的图片。在商店列表、详情卡片和面板按钮上，它会代替 `icon` 的名称被绘制出来。

**徽标绝不会从任何地方获取。**它随插件一起分发，所以屏幕上画出来的就是用户安装的那张图；而且给插件配上徽标，并不会让作者知道是谁安装的。

| 插件类型 | 徽标在哪里 |
|---|---|
| 文件夹 —— `node`、`python`、`executable`，或链接用于开发的文件夹 | `"logo": "logo.png"`，插件文件夹内的一个文件 |
| 单个 `wasm` 模块 —— 一切来自商店的插件 | 模块自己带着图片。不使用 `logo`；见 [Rust 与 WebAssembly](/docs/plugin-rust#徽标) |

作为清单字段，`logo` 只是一个文件名。走出插件文件夹的路径（`../`）、绝对路径、带协议的值（包括 `https://` 地址），以及超过 400 个字符的值都会被拒绝，插件继续使用它的 `icon`。

模块携带的图片必须是 **PNG、JPEG、GIF 或 WebP**，并且**按字节而不是按名称**核对确实是该格式，最多 512 KB。其他一概不算徽标，插件继续使用它的 `icon`。

**模块的徽标绝不会是 SVG**，无论它叫什么名字。因为 SVG 是文档而不是图片：渲染器会解析其中写着的地址，若那里写的是本地路径，该文件就会被打开并画出来。插件不能以徽标之名把用户自己的文件放上屏幕。

## 环境变量

以程序方式运行的插件（`node`、`python`、`executable`）会收到以下环境变量。`wasm` 插件一个都收不到，它把需要的东西放在[自己的存储](/docs/plugin-permissions)里。

| 变量 | 含义 |
|---|---|
| `AGENTTY_PLUGIN_ID` | 插件的 id |
| `AGENTTY_PLUGIN_DIR` | 插件文件夹 |
| `AGENTTY_PLUGIN_DATA` | 存放设置与缓存的私有文件夹 |
| `AGENTTY_VERSION` | Agentty 版本 |
| `AGENTTY_LANGUAGE` | 用户语言（`en`、`ko`、`ja`、`zh`） |
| `AGENTTY_BIN` | `agentty` 命令行工具的路径 |

需要持久化的内容请放在 `AGENTTY_PLUGIN_DATA` 里，或者放进两种插件都适用的 `storage/*`。
