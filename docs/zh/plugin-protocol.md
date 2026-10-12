---
title: 插件协议
description: SDK 背后的 JSON-RPC 线上格式、WebAssembly 模块 ABI，以及全部消息、限制与错误码。
---

不借助 SDK 编写插件时用到的线上格式。[Node.js SDK](/docs/plugin-sdk) 和 [Rust SDK](/docs/plugin-rust) 都已经封装了全部内容；无论哪种方式，建议先读[快速开始](/docs/plugin-quickstart)。

API **版本 1** 是本页中除 `host/timer` 和 `pane/status` 之外的全部内容，这两者属于**版本 2**；[UI 树](#ui)中标为 API 4 的面板元素属于**版本 4**；`tools/call` 属于**版本 5**。

## 传输方式

Agentty 以插件文件夹作为工作目录启动程序：

| `runtime` | 启动命令 |
|---|---|
| `node` | `node <main>` —— 来自登录 shell `PATH`、Homebrew、Volta 或 nvm 的 Node.js |
| `python` | `python3 <main>` |
| `executable` | `<main>` |
| `wasm` | 无 —— `<main>` 是 Agentty 自己运行的 WebAssembly 模块，见 [WebAssembly 插件](#webassembly)|

消息是 [JSON-RPC 2.0](https://www.jsonrpc.org/specification) 对象，**每行一个**，UTF-8 编码，经由 stdin（Agentty → 插件）和 stdout（插件 → Agentty）传递。超过 16MB 的行会被拒绝。stdout 上非 JSON 的内容会被记录并忽略，stderr 进入插件日志。

当 stdin 关闭或收到 `shutdown` 时请退出。`shutdown` 后 1.5 秒仍在运行会收到 `SIGTERM`，再过 1.5 秒收到 `SIGKILL`。Agentty 退出时两者会立即依次发出。

## Agentty → 插件

| 消息 | 类型 | `params` |
|---|---|---|
| `initialize` | 请求 —— 需要应答 | `{ apiVersion, agentty: { version }, plugin: { id, name, version, dir, dataDir }, language, context }` |
| `command/execute` | 通知 | `{ command, args, context }` |
| `panel/open` · `panel/close` | 通知 | `{ context }` |
| `ui/event` | 通知 | `{ element, event, value?, item?, action?, context }` |
| `context/changed` | 通知 | `{ context }` |
| `url/open` | 通知 | `{ path, query, url, context }` |
| `pane/status` | 通知 | `{ paneId, status, running, agent, title, cwd }` —— 这个插件启动的某个窗格状态变了（`workspace.read`，API 2） |
| `tools/call` | 请求 —— 需要应答 | `{ name, arguments, context }` —— 某个 AI 智能体在调用工具（`mcp.tools`，API 5） |
| `shutdown` | 通知 | `{}` |

`initialize` 最先到达，紧接着是启动该插件的那件事（命令、打开面板或链接）。对 `initialize` 用任意结果应答即可，例如 `{}`。

## 插件 → Agentty

需要结果或错误时作为请求发送（带 `id`），不关心结果时作为通知发送（不带 `id`）。

| 方法 | 权限 | `params` | 结果 |
|---|---|---|---|
| `ui/setPanel` | | `{ tree }` | `null` |
| `ui/showPanel` | | `{}` | `null` |
| `ui/notify` | | `{ message, kind }` —— `info`、`success`、`warning`、`error` | `null` |
| `ui/setBadge` | | `{ text }`，最多 8 个字符 | `null` |
| `context/get` | | `{}` | 上下文 |
| `host/info` | | `{}` | `{ version, apiVersion, language, uiFeatures }` |
| `host/openUrl` | | `{ url }` —— http/https | `null` |
| `host/copy` | | `{ text }` —— 最多 100,000 个字符 | `null` |
| `host/timer` | | `{ ms }` —— API 2 | 到点后返回 `{ elapsedMs }` |
| `host/revealPath` | `workspace.read` | `{ path }` —— 存在的绝对路径 | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, cwd?, submit? }` | `{ status: "asked" }` 或 `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` —— 不带 `paneId` 时为聚焦窗格 | `{ paneId }` |
| `terminal/setProxy` | `terminal.proxy` | `{ port, token }`（`token`：16–128 个字母、数字、`-`、`_`）或 `{ port: null }` —— 之后打开的终端经过插件在 127.0.0.1 上的代理 | `null` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` —— 默认 200，最大 2000 | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes }]` |
| `net/fetch` | `net.request` | `{ url, method?, headers?, body?, timeoutMs?, proxy? }` | `{ status, statusText, url, headers, body, truncated, binary, bytes, durationMs }` |
| `storage/get` | | `{ key }` | `{ key, value }` —— 未设置时 `value` 为 null |
| `storage/set` | | `{ key, value }` —— 传 null 即删除 | `null` |
| `storage/keys` | | `{}` | `[key]` |

到达速度快于每 700ms 一条的 `ui/notify` 会被丢弃（但仍正常应答），每秒发送超过 240 条消息的插件会被停止。上下文字段受插件权限限制。

### 接触网络

`net/fetch` 是插件接触网络的唯一途径。Agentty 给它划下的边界——方法、请求头、大小、超时、重定向——都在[权限](/docs/plugin-permissions#net-request)里。一句话说完：请求里不会带上任何属于你的东西，没有 cookie，没有已保存的凭据，只有插件自己放进去的内容。

### 回应智能体的工具调用 (API 4)

当某个 AI 智能体调用你的工具时，Agentty 用 `{ name, arguments, context }` 发送 `tools/call`。用字符串、任意 JSON 值或 MCP 格式的内容 `{ "content": [{ "type": "text", "text": "…" } | { "type": "image", "data": "<base64>", "mimeType": "image/png" }], "isError": false }` 应答。其他内容类型会被丢弃。或者用 JSON-RPC 错误应答；其消息会作为错误传给智能体。

插件必须在 120 秒内应答。同一个插件最多有 8 个调用在等待。参数最多 256 KB；传给智能体的文本在 100,000 字符处被截断。

### 等待，以及听到智能体干完了

`host/timer` 是插件等待的方式：一个到点才被响应的请求。最短 100 ms，最长 1 小时，同时 8 个。模块只在处理消息时运行，所以这就是它回到「稍后再说」的全部办法——它得到的就只有那个响应，因此这不是后台运行的手段。

`pane/status` 是插件听到自己启动的智能体干完了的方式。插件从 `prompt/inject` 的响应（`{ status: "sent", paneId }`）里得知窗格 id；Agentty 记得哪个插件启动了哪个窗格，并且只在那个窗格状态变化时告诉那个插件——`working`、`idle`、`finished`、`permission`、`question`、`interrupted`、`exited`，以及最后一次 `closed`。同时最多跟踪 32 个窗格。

用户自己放下的提示词同样会被跟踪：`target: "ask"` 因为还没有窗格，所以返回不带窗格 id 的 `{ status: "asked" }`，但用户选定的那个会话一样会被盯着——它的第一条 `pane/status` 就是插件得知自己变成了哪个窗格的地方。同时有多个问题悬着的插件，靠自己给提示词起的 `title` 来区分。

不管界面有没有在绘制，状态都会送到。被别的窗口挡住的窗口、锁屏时的窗口都不会绘制，而一个在等智能体的插件，不该在等用户回来。[AgentOS 插件](/docs/plugin-agentos)就建立在这两者之上。

### 记住东西

`storage/*` 是插件在两次运行之间记住东西的方式：它自己文件夹里的一个 JSON 文档（`<数据目录>/plugin-data/<plugin>/storage.json`，以 `0600` 创建），按键读写。键由小写字母、数字、`.`、`-` 和 `_` 组成，最多 64 个，共 1 MB。以进程方式运行的插件可以自己写文件；WebAssembly 插件没有文件，所以这是它留下任何东西的唯一办法。

### 错误码

| 代码 | 含义 |
|---|---|
| `-32601` | 未知方法 |
| `-32602` | 参数无效 —— UI 树有误、窗格不存在等 |
| `-32001` | 缺少权限，或因链接到达而被拒绝 —— 直到插件重启 |
| `-32002` | 不可用 —— 没有打开的窗口、尚无会话 |

## 交互示例

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":1,"plugin":{"id":"hello"},"language":"en","context":{}}}
→ {"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}
← {"jsonrpc":"2.0","id":1,"result":{}}
← {"jsonrpc":"2.0","id":1,"method":"ui/setPanel","params":{"tree":{"type":"column","children":[{"type":"button","id":"go","label":"Go"}]}}}
→ {"jsonrpc":"2.0","id":1,"result":null}
→ {"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"click","context":{}}}
← {"jsonrpc":"2.0","id":2,"method":"prompt/inject","params":{"text":"Hello","target":"ask"}}
→ {"jsonrpc":"2.0","id":2,"result":{"status":"asked"}}
```

`→` 表示 Agentty 发给插件，`←` 表示插件发给 Agentty。请求 id 按方向各自计数。

## WebAssembly 插件

把 `runtime` 设为 `"wasm"`，`main` 就是一个 Agentty 在自己内部用解释器运行的 `.wasm` 模块——一个文件在 macOS、Windows 和 Linux 上都能跑。

模块只能调用 Agentty 交给它的函数。没有文件，没有套接字，没有环境变量，没有进程，除了一个计数器也没有时钟，所以 WebAssembly 插件不管怎么写都读不到 `~/.agentty`、用户的项目或凭据。它想从 Agentty 那里得到什么，就用与进程型插件写到 stdout 相同的消息去要，清单里的权限也照样检查。

模块导出：

| 导出 | 含义 |
|---|---|
| `memory` | 它的线性内存 —— Rust 或 C 模块的标准导出 |
| `agentty_alloc(len: i32) -> i32` | 一块 `len` 字节的缓冲区，供 Agentty 写入消息 |
| `agentty_on_message(ptr: i32, len: i32)` | 来自 Agentty 的一条 UTF-8 JSON 消息 |

并从名为 `agentty` 的模块导入：

| 导入 | 含义 |
|---|---|
| `send(ptr: i32, len: i32)` | 向 Agentty 发送一条 UTF-8 JSON 消息 |
| `log(ptr: i32, len: i32)` | 写入插件日志的一行 |
| `now_ms() -> i64` | Unix 纪元以来的毫秒数 |

消息就是与 stdio 相同的 JSON-RPC 对象，每次调用一条，不带换行。**导入了别的东西的模块不会被加载。** 超过 64 MB 的模块同样会被拒绝，内存上限为 64 MB，每条消息还有工作量预算：不返回的插件会以「未能及时完成」停止，在处理单条消息期间发出超过 256 条消息的插件也会被停止——`send` 与 `log` 合并计数，所以只打日志的循环也不是免费的。

[Rust SDK](/docs/plugin-rust) 把这一切都藏了起来。

## UI 树

每个节点都是带 `type` 的对象：

```
column   { children, gap? }                gap: none | small | medium | large
row      { children, gap?, wrap? }
section  { title, children }
text     { text, style? }                  style: body | title | muted | small | code | error | success
button   { id, label, icon?, variant?, disabled? }   variant: primary | secondary | ghost | danger
input    { id, placeholder?, value?, rows?, mono? }
         mono: 等宽字体 (API 4)
         completions (API 4): [{ label, insert?, detail? }] 正在输入的词的补全建议，
         前面的 {{ 和后面的 }} 一并替换
         rows > 1：该行数的文本域（最多 24）；回车换行，粘贴保留换行
list     { id, items, empty? }
         items: [{ id, title, subtitle?, detail?, icon?, tone?, actions?: [{ id, label?, icon?, tooltip? }] }]
         depth: 树的缩进 0–8，tag: 标题前的短标签（≤ 8 个字符），颜色为 tagTone (API 4)
choice   { id, options: [{ value, label }], value? }
toggle   { id, label, value? }
badge    { text, tone? }                   tone: neutral | info | success | warning | error
spinner  { text? }
divider  {}
flow     { id, steps: [{ id, title, subtitle?, icon?, state?, selected?, side? }] }
         state: off | on | active | done | error — 自动化的步骤，自上而下
popover  { id, title, children }           面板旁边的卡片；树中第一个会被打开

API 4:
card     { children, title?, subtitle?, icon?, tone? }   浮起的方框；tone 决定图标和边框颜色
grid     { children, columns?, gap?, widths? }      等宽的列（1–6，默认 2），放不下时换到下一行
         widths: 每列 "240px"（固定）或 "2"（比例）— 代替 columns
         id?, resizable? (API 4): 拖动第一个固定列调整宽度，结束后发送 resize（宽度）
         fill: 占满剩余高度，每列单独滚动 (API 4)
tabs     { id, tabs: [{ id, label, icon?, badge?, closable? }], value, children }
         closable: 关闭按钮，点击发送 close
         children：只放选中标签页的内容
table    { id, columns: [{ label, align?, grow? }], rows: [{ id, cells, tone? }], empty?, selected? }
         align: start | center | end；grow：所占宽度比例（1–12）；每列一个单元格
keyValue { items: [{ label, value, tone?, mono? }] }
stat     { label, value, detail?, icon?, tone? }
progress { value, label?, detail?, tone? }  value 取 0 到 1；detail 代替百分比显示
callout  { text, title?, icon?, tone? }     tone 默认为 info
select   { id, options: [{ value, label }], value?, placeholder?, disabled? }
checkbox { id, label, value?, description?, disabled? }
code     { text, language?, title? }       按 language（rust、json、ts、sh 等）着色，带复制按钮

API 5:
image    { src?, id?, alt?, width?, height?, fit?, caption?, placeholder?, drop?, into? }
                                           插件自己文件夹（files/*）中的图片：仅 PNG、JPEG、WebP、GIF，不绘制 SVG；
                                           高度默认 240；fit: contain | cover；没有 src 时为空位；drop（与 id 一起，
                                           需要 files 权限）接收从 Finder 拖放的图片
graph    { id, nodes, edges?, height?, movable?, fill? }
                                           把放在画布 x, y 处的节点卡片用连线连起来（像图像流水线那样）；fill 时占满面板剩余高度；
                                           可在其中滚动与缩放（− % + 适应，⌘/Ctrl + 滚动）
         nodes: [{ id, title, subtitle?, icon?, tone?, x, y, width?, inputs?, outputs?, children?, state?, selected? }]
         edges: [{ from: { node, port }, to: { node, port }, tone?, idle? }]
```

事件：`button` 发送 `click`；`input` 发送带 `value` 的 `change` 与 `submit`；`list` 发送带 `item` 的 `select`，行内按钮发送带 `item` 和 `action` 的 `action`；`choice` 发送带选项值的 `change`；`toggle` 发送带新布尔值的 `change`。

列表项的 `tone` 决定其图标颜色，取值与 `badge` 相同。

`flow` 以步骤 id 作为 `item` 发送 `select`；`popover` 的关闭按钮发送 `close`。自 API 4 起：`tabs` 发送带标签页 id 的 `change`；`table` 以行 id 作为 `item` 发送 `select`；`select` 发送带选项值的 `change`；`checkbox` 发送带新布尔值的 `change`。自 API 5 起：`image` 发送 `click`，以及带 `[{ path, name, size }]` 的 `drop`（与 `files/pick` 一样，是复制到插件文件夹中的路径）；`graph` 在点击节点标题时以节点 id 作为 `item` 发送 `select`，设置 `movable` 时，拖动标题后以节点为 `item`、`{ x, y }` 为 `value` 发送 `move` — 请记住这个位置并发回。

使用 API 4 元素的插件需在清单中写上 `"apiVersion": 4`。当前运行的 Agentty 能绘制哪些元素，见 `host/info` 的 `uiFeatures`。什么内容该用哪个元素，见 [设计面板](/docs/plugin-ui-guide)。

## 不用 Agentty 也能测试

`node`、`python` 和 `executable` 插件就是读取 stdin、写入 stdout 的普通程序，因此可以从测试中直接驱动：先写一个 `initialize` 请求，再发送你想验证的通知，然后对插件写回的 JSON 做断言。

`wasm` 模块也是同样的驱动方式，任何能提供上面那三个 import 的 WebAssembly 运行时都可以。
