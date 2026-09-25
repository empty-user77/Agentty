---
title: 发布插件
description: 把插件登记到市场，或者自己以 Git 仓库、文件夹的形式分发。
---

把插件交到别人手里有两条路，能走哪一条取决于它是哪种插件。

| | |
|---|---|
| **插件市场** | 源码公开的 WebAssembly 模块。出现在**插件 → 市场**，一键安装，并用校验和核对。 |
| **自己分发** | 任何插件，包括以程序方式运行的。用户从自己挑定的 Git 仓库或文件夹安装。 |

## 插件市场

[Agentty-Marketplace](https://github.com/empty-user77/Agentty-Marketplace) 就是 Agentty 读取的那份清单。插件通过 pull request 添加。Agentty 自己的插件也在那里，条件完全一样——应用里没有内置任何插件。

清单里的插件是**一个源码公开的 WebAssembly 模块**。规则只有这一条，而且两半都要紧：

- **必须是 WebAssembly**，因为 Agentty 自己运行它，它只能碰到协议给的东西。以程序方式运行的插件拥有你拥有的一切，Agentty 不会从互联网上的清单里安装那种东西。
- **源码必须公开**，因为清单里的模块是二进制。任何人都能读它由什么构建而来，并且重新构建一遍。

### 安装时会发生什么

1. Agentty 通过 HTTPS 读取 `index.json`。每一条都会在这里重新检查——id、文案、权限、模块来自哪个主机。没通过的条目会被排除在清单之外，而不是照样显示。
2. 插件页面会展示它是什么、源码在哪、许可证、模块大小和校验和，以及在权限之下用完整句子写明的**它能做什么**。
3. 按下**安装**后，Agentty 下载模块，核对长度是否等于 `module.size`，与校验和比对，并确认这些字节确实是一个 WebAssembly 模块。三者都对上之前，插件文件夹里不会进入任何东西。

### 提交一个插件

1. **构建并发布模块。** 通常放在插件仓库的 GitHub release 里。

   ```bash
   cargo build --release --target wasm32-unknown-unknown
   cp target/wasm32-unknown-unknown/release/your_plugin.wasm your-plugin.wasm
   shasum -a 256 your-plugin.wasm
   wc -c your-plugin.wasm
   ```

2. **写条目。** 把 `plugins/_template.json` 复制成 `plugins/<你的插件 id>.json`。id 要和文件名以及 `agentty-plugin.json` 里的 `id` 一致。

   ```json
   {
     "id": "hello-world",
     "name": "Hello World",
     "version": "0.1.0",
     "publisher": "Your Name",
     "description": "One sentence about what it does.",
     "icon": "sparkles",
     "license": "MIT",
     "source": "https://github.com/you/agentty-hello-world",
     "build": {
       "repository": "https://github.com/you/agentty-hello-world",
       "rev": "3f2b1c9e4a7d05b8c6e1f0a2d4b83c7e9015d6af",
       "path": ".",
       "toolchain": "1.98.1",
       "artifact": "target/wasm32-unknown-unknown/release/hello_world.wasm"
     },
     "keywords": ["example"],
     "apiVersion": 1,
     "surface": "sidebar",
     "mode": "push",
     "permissions": [],
     "module": {
       "url": "https://github.com/you/agentty-hello-world/releases/download/v0.1.0/hello-world.wasm",
       "sha256": "…64 位十六进制…",
       "size": 93292
     }
   }
   ```

3. **先检查，再开 pull request。** CI 会跑同样的检查，并且只有条目、它所指向的模块，以及重新构建你的源码这三者一致时才会通过。

   ```bash
   python3 scripts/validate.py              # 每个条目的结构、主机、权限和 build 块
   python3 scripts/validate.py --download   # 同时下载每个模块并核对校验和
   python3 scripts/validate.py --source     # 同时检查源码是否人人可读
   python3 scripts/verify_build.py          # 重新构建你的源码，与校验和比对
   ```

`verify_build.py` 需要 Docker：模块在按摘要固定的 `rust` 镜像中构建，因此字节不依赖于构建它的机器。这正是 CI 能得出与你相同校验和的原因。

### 条目里的字段

| 字段 | |
|---|---|
| `id` | 2–40 个字符的 `a-z 0-9 -`；文件名为 `plugins/<id>.json` |
| `name`、`version`、`description` | 在 Agentty 中展示。`name` 最多 60 字符，`description` 最多 300，`version` 为 `major.minor.patch` |
| `publisher`、`license` | 作者，以及许可证 |
| `source` | 模块由之构建的公开仓库——**必填**。可以在 `github.com`、`gitlab.com`、`codeberg.org` 或 `git.sr.ht`，不一定要是 GitHub。它必须与 `build.repository` 是同一个仓库，这样条目链接的代码就是它实际发布的代码 |
| `build` | **必填** —— 如何再次构建该模块：`repository`（克隆地址）、`rev`（完整的 40 位提交，不能是之后可被移动的标签或分支）、`path`（插件在仓库中的目录，或 `.`）、`toolchain`（市场用来构建的 Rust 版本）和 `artifact`（构建写出的 `.wasm`，相对于 `path`）。其中没有任何一项是命令 —— 执行什么固定写在 `scripts/verify_build.py` 里 |
| `homepage`、`keywords`、`icon` | 可选；图标取自 Agentty 的图标集 |
| `apiVersion` | 模块面向的插件协议版本；为 `1` 时可省略 |
| `surface` | 图标的位置：`sidebar`、`pane`（默认）或 `status` |
| `mode` | 面板的打开方式：`push`（默认）、`overlay`、`window` 或 `full` |
| `permissions` | 它申请的权限——安装前会展示，要求变多的更新会再展示一次 |
| `module.url` | `github.com`、`raw.githubusercontent.com` 或 `objects.githubusercontent.com` 上的 `https://` 地址。把版本放进路径，release 就无法被悄悄替换 —— 这是惯例，不是校验项 |
| `module.sha256` | 校验和；不匹配的下载会被拒绝，即使匹配，不是 WebAssembly 模块的字节也会被拒绝 |
| `module.size` | 以字节为单位的精确长度，最大 8 MB。它不是上限而是精确值 —— 长度不符的下载会被拒绝，所以每次构建都会变 |

上架的插件不会在条目里携带徽标。如果想用图片代替图标，请把它放进模块的 `agentty.logo` 段 —— 这样它就和其余部分一样被 `module.sha256` 覆盖，而且别人安装插件时不会向你的服务器发出任何请求。见[徽标](/docs/plugin-rust#徽标)。

### 什么会被拒绝

- CI 按 `build.rev` 重新构建 `build.repository` 也得不到同样字节的模块，或者谁都读不到的仓库。
- `build.path` 处的清单与条目不一致 —— id、版本、`apiVersion` 或权限列表不同。
- 校验和与该 URL 提供的内容不一致。
- 申请了用不到的权限，或者描述里没写清楚拿这些权限做什么。
- 冒充别的插件、别的发布者，或者冒充 Agentty 本身。

### 更新

改掉 `version`、`module.url`、`module.sha256` 和 `module.size`，再开一个 pull request。所有装了它的人都会收到更新提示。

比已安装版本要求**更多**权限的更新，会说明多要了什么，并且需要再按一次。**全部更新**不会悄悄替你拿下这些，而是把它们留下并告诉你留了几个。所以扩大权限的代价，是失去那些不会再看一眼的用户——把描述写成值得他们再看一眼的样子。

如果新版本用到了只有较新 Agentty 才有的东西，请把 `apiVersion` 一起调高。用旧版 Agentty 的人就会保留手里的版本并被提示更新，而不是拿到一个应用跑不起来的模块——Agentty 也不会把自己跑不了的版本算作已安装版本的更新。

删掉条目后，Agentty 不再提供这个插件。已经装了的人那里仍然保留，他们可以在插件页面自行删除。

> [!NOTE]
> `AGENTTY_MARKETPLACE_INDEX` 可以让 Agentty 指向另一份清单，方便你在编写清单时试用。模块可以由仓库的 release 提供，也可以来自与清单相同的主机。

## 自己分发

以上都不是使用插件的必要条件。只给自己或公司内部用的插件，完全不必经过那份清单。

### 作为 Git 仓库

把 `agentty-plugin.json` 放在仓库**根目录**。用户在*自己动手*下粘贴 `https://` 地址，然后按**从 Git 安装**。

```
your-plugin/
├── agentty-plugin.json
├── main.mjs
├── agentty-plugin.mjs     一并打包的 SDK
├── README.md
└── LICENSE
```

> [!IMPORTANT]
> **把运行所需的一切都打包进去，包括 SDK 和任何 `node_modules`。** Agentty 不会执行 `npm install`，它原样启动你的入口文件。带着未打包依赖的插件会在用户机器上以 module-not-found 报错，只留在日志里。

能不用依赖就不用。SDK 本身没有依赖，正是出于这个原因。

### 作为文件夹

**从文件夹安装…** 会把文件夹复制到 `~/.agentty/plugins/`。解压后是正确名字的 zip 也一样——文件夹名必须等于清单里的 `id`。

## 版本

`version` 是 `major.minor.patch`。每次发布都要抬高；Agentty 靠比较它来判断已安装的插件有没有更新。

保持清单和代码同步：不再使用的权限应当从清单里去掉，改了名的命令不该留在 `contributes` 里。

## 下一步

- [Rust 与 WebAssembly](/docs/plugin-rust) —— 构建模块
- [清单参考](/docs/plugin-manifest) · [权限](/docs/plugin-permissions)
