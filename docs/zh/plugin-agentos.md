---
title: AgentOS 插件
description: 一种不自己干活、而是通过 Agentty 的智能体推进工作的插件——它的形态、必须遵守的规则，以及为什么它不需要浏览器。
---

**AgentOS** 是一种不自己干活、而是通过 Agentty 的智能体推进工作的插件。它装着某个行当的技能和规则——写博客、做营销、一个内容创作者的一周——然后把一件事一步步送进这些规则里。真正动笔的是智能体，而那些会话你可以读，也可以随时接手。

它和别的插件没有区别：一个 WebAssembly 模块，同样的权限，同样的面板。让它成为 AgentOS 的，是它装了什么，以及它怎么用 `prompt.inject`。

```
┌─ 插件（一个 .wasm 模块）───────────────────────────────────────┐
│  技能   每一步的提示词，为这个行当而写                         │
│  规则   一步不能做什么，以及「完成」是什么意思                 │
│  进度   下一步是哪一步、每一步产出了什么、用户批准了什么       │
└────────────────────────────────────────────────────────────────┘
          │ prompt/inject                       │ session/get
          ▼                                     ▼
   一个 Claude Code 或 Codex 会话          智能体写回来的内容
```

一步是这样的：给智能体发一条提示词，等它停下来，读它的产出，然后做判断。插件把状态放在 `storage/*` 里，所以重启之后进度还在，并把它画在面板上——步骤列表、当前位置、返回的内容，以及批准并继续的按钮。

AgentOS 不是新的运行时，也不是新的权限。它只是一种大部分内容都是提示词的插件形态。

## 为什么它不需要浏览器

做一个会发帖的插件，最直接的办法是给插件一个浏览器。那等于把你已登录的会话交给每一个插件，这里不这么做。

**插件没有浏览器，也没有网络。** 它请智能体去做。Agentty 早就给每个智能体会话配了自己的浏览器，而那正是你**已经登录**的那个浏览器，由智能体本来就能用的 shell 驱动：

```bash
agentty browser navigate <url>        agentty browser text [selector]
agentty browser click <selector>      agentty browser type <selector> <text>
agentty browser elements              agentty browser screenshot <path.png>
```

所以一个会发帖的工作流，其实是一条写明打开哪个页面、输入什么的提示词，发进一个你正看着的会话。插件可以往网站上发东西，却从来没被交出过你的登录会话。

真正要紧的规则——不要登录、不要安装任何东西、停下来说清楚看到了什么、阅读时不要动手——就写在那条提示词里。工作流把它们和 glossary 放在一处，这样没有哪一步能绕开它们被写出来。

## Agentty 补上的部分

模块没有时钟也没有循环：它只在处理消息时运行。有两条消息让其余一切成为可能，两者都需要 `apiVersion: 2`。

### `host/timer` —— 等待

| 方法 | 权限 | `params` | 结果 |
|---|---|---|---|
| `host/timer` | | `{ ms }` | 到点后返回 `{ elapsedMs }` |

一个稍后才被响应的请求。最短 100 ms，最长 1 小时，每个插件同时 8 个；被停止或重启的插件会丢掉正在等的那些。它不是后台运行的手段——插件得到的就只有那个响应。

### `pane/status` —— 听到智能体干完了

| 消息 | 类型 | `params` |
|---|---|---|
| `pane/status` | 通知 | `{ paneId, status, agent, title?, cwd?, running }` |

当**这个插件启动的**窗格状态发生变化时发送。插件从 `prompt/inject` 的响应（`{ status: "sent", paneId }`）里得知窗格 id；Agentty 记得哪个插件启动了哪个窗格，并且只告诉那个插件。它需要 `workspace.read`，也就是本来就意味着「查看智能体状态」的那个权限。同时最多跟踪 32 个窗格。

不管界面有没有在绘制，状态都会送到。被别的窗口挡住的窗口、锁屏时的窗口都不会绘制，而一个在等智能体的插件，不该在等你回来。

你自己放下的提示词同样会被跟踪。`target: "ask"` 因为还没有窗格，所以返回不带窗格 id 的 `{ status: "asked" }`，但你选定的那个会话一样会被盯着——它的第一条 `pane/status` 就是插件得知自己变成了哪个窗格的地方。

## 怎么写

每个 AgentOS 共有的那部分在 Rust SDK 的 `agentty_plugin::agentos` 里。一个工作流是一串步骤；一步则是一条提示词、一项对返回内容的检查，以及进入下一步前是否要问用户。

```rust
static BLOGGER: Workflow = Workflow {
    id: "blogger",
    title: "Blog post",
    agent: Some("claude"),
    // 多个步骤共用的提示词片段——驱动浏览器的规则、文风要求等。
    glossary: &[],
    steps: &[
        Step { id: "outline", title: "Outline", prompt: OUTLINE, check: has_headings, approval: Approval::Auto },
        Step { id: "draft", title: "Draft", prompt: DRAFT, check: long_enough, approval: Approval::Auto },
        Step { id: "edit", title: "Edit", prompt: EDIT, check: no_placeholders, approval: Approval::Ask },
        Step { id: "save", title: "Save", prompt: SAVE, check: names_a_file, approval: Approval::Ask },
    ],
};
```

`{input}` 是这次运行开始时输入的内容，`{step.<id>}` 是前面某一步的产出；两者都会连同工作流的 glossary 一起，在发出提示词之前被填好。

`Approval::Auto` 自动开始下一步，`Approval::Ask` 则把返回的内容摆出来，等你按**继续**。

### 运行过程

每一次状态转移，都是插件本来就会收到的消息：

| 发生了什么 | 运行器做什么 |
|---|---|
| 用户按下**开始** | 用 `target: "newTab"` 把第一步的提示词 `prompt/inject` → 记住 `paneId` |
| `pane/status` 说那个窗格在 `working` | 记下提示词已被接手 |
| 之后它变成 `finished` 或 `idle` | 等 2.5 秒，看这次停顿是不是真的 |
| 仍然停着 | `session/get`，然后跑这一步的检查 |
| 在这 2.5 秒里又开始干活了 | 它刚才说的并不是答案 —— 回到等待 |
| 检查通过 | 留下产出，展示它或发出下一步 |
| 检查没过 | 把缺了什么发回给智能体——最多三次，之后停下并说明原因 |
| 智能体开口问什么，或窗格没了 | 运行停下并告知 |
| Agentty 重启了 | 从 `storage` 读回进度，重新去问那个会话 |

那 2.5 秒不是拍脑袋定的。智能体在两次工具调用之间会短暂空闲，在那一刻读会话，拿回来的是半句话加上它正要运行的工具名。把它当作答案的那一步，等于什么都没读就往下走了。

`working` 那一行存在也是同样的道理。窗格从打开的那一刻起，在智能体接手提示词之前，一直是 `idle`。所以单看 `idle` 从来不等于「干完了」——只有在看到那个窗格至少 `working` 过一次之后，运行才相信这次停顿。

## 两条规则

有两件事工作流说了不算，因为 AgentOS 是一个代表你去跟智能体打交道的插件。

> [!IMPORTANT]
> **最后一步之前一定会问你，不管那一步怎么写。** 最后一步是对外界动手的那一步——发布、推送、发送——而你在它运行前看到的东西，就是它将要处理的东西。把那一步标成自动的工作流，等于一个趁你不看时往你账号里写东西的插件。

**会拿没人读过的答案去发布的工作流，在插件启动时就会被拒绝。** `Workflow::checked()` 在 `init` 里就说出来，而不是等到要发布的那一刻。步骤少于两个、步骤 id 重复、glossary 里有没人用的条目，同样会被拒绝。

还有两件是工作流自己要做对的：

- **用户在场。** 一步的提示词进入的是他们能读的会话、能接手的标签页。
- **链接不等于一次运行。** 被链接触发的插件不能往终端里打字，它的提示词在这个插件运行期间会一直走**发送到…**。这样启动的 AgentOS 会先问一句——而它照样跟着你选定的会话，所以问这一下只花一个回合。

## 两个示例

两个都在[插件市场仓库](https://github.com/empty-user77/Agentty-Marketplace)里，连源码和校验和一起。

**Blogger AgentOS** —— 提纲、草稿、润色、保存。大约 180 行，大部分是提示词。检查也是真的在查：提纲里三个小节、草稿三百个词、不留 `TODO`。想自己写之前，先读这个。权限是 `prompt.inject`、`session.read`、`workspace.read`。

**Social AgentOS** —— 三个工作流，都通过你已登录的浏览器进行。

| | 步骤 |
|---|---|
| **发到 X** | 读一读大家在说什么 → 起草 → **你来读** → 发出去 |
| **在 X 回复** | 找值得参与的对话 → 起草回复 → **你来读** → 发送 |
| **Instagram 文案** | 读这个账号怎么写 → 文案 → **你来读** → 保存，并复制到剪贴板 |

它不会发你没读过的东西，不会登录（遇到登录页或验证码的提示词会停下来告诉你看到了什么），阅读时不会点赞、转发、回复或关注，一次运行最多发五条回复。Instagram 需要手动选图，所以最后一步到此为止：把文案写进文件、放进剪贴板，然后替你打开 Instagram。

它只要 `prompt.inject`、`session.read` 和 `workspace.read`，再无其他。它没有地方放密码，因为它从来就没有过。

## 还没有的东西

- **技能住在模块里。** 让用户改某一步提示词的插件，会把改动存进 `storage`，对一台机器来说够用了；把一套技能分享给别人是市场的问题，不是协议的问题。
- **同时多个智能体。** `prompt/inject` 想开多少窗格都行，`pane/status` 也会逐个报上来，但没有办法说「这三个算一步」——插件只能自己记住这些 id。
- **花费。** 一次运行是好几个智能体会话。Agentty 的用量页面能看到花了多少，但插件问不到。
- **按时间运行。** 运行要你按下**开始**才开始。一个每个工作日早上发帖的 AgentOS 需要 Agentty 替它启动，而 `host/timer` 只在插件运行时才会被响应，插件又只在 Agentty 开着时才运行。

## 下一步

- [Rust 与 WebAssembly](/docs/plugin-rust) —— 写 AgentOS 用的 SDK
- [插件协议](/docs/plugin-protocol) · [权限](/docs/plugin-permissions)
