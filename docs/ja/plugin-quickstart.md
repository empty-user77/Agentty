---
title: プラグイン クイックスタート
description: AI で、Rust で、あるいは JavaScript で、動く Agentty プラグインを作ってインストールする。
---

プラグインはマニフェストとプログラムが入ったフォルダです。ターミナルの横に**パネル**を置き、エージェントペインの上に**ボタン**を足し、**コマンドパレット**に項目を入れ、テキストを**プロンプト**としてエージェントに渡せます。

2 種類あり、先に選んでおくとよいものです。

| | |
|---|---|
| **WebAssembly**（`runtime: "wasm"`） | `.wasm` ファイル 1 つ。ふつうは Rust でビルドします。Agentty 自身が実行するのでプロトコルが与えたものにしか触れられず、利用者のマシンにインストールするものもありません。[マーケットプレイス](/docs/plugin-publishing)が受け付ける唯一の形式です。 |
| **プログラム**（`node`, `python`, `executable`） | 利用者の権限で動き、利用者が起動する他のものと同じアクセス権を持ちます。書くのは速く、利用者が自分で選んだフォルダや Git リポジトリからインストールします。 |

## AI で作る

ほとんどのプラグインはこの方法で書かれます。アクティビティバーのパズルアイコンで**プラグイン**を開き、*自分で作る*の下に名前と何をするプラグインかを書いて、**Claude Code で作成してビルド**を押します。

Agentty がテンプレートからプラグインを作り — SDK、型定義、開発者ガイド、指示の入った `CLAUDE.md` / `AGENTS.md` — そのフォルダで Claude Code を開きます。**AI プロンプトをコピー**を押せば、同じプロンプトを他のエージェントでも使えます。

### エージェントに何を伝えるか

テンプレートがすでにガイドを抱えているので、役に立つプロンプトはプラットフォームの説明ではなく、作りたいプラグインそのものです。

```text
まず PLUGIN_GUIDE.md と agentty-plugin.d.ts を読んでください。

フォーカス中のペインのフォルダにある Git ブランチを一覧するパネルを作ってください。
各行にブランチ名と、main からどれだけ進んでいる/遅れているかを表示します。
行をクリックしたら、そのブランチで何が変わったかの要約をエージェントに依頼します。

権限は prompt.inject と workspace.read だけを要求し、それ以外は要求しないでください。
```

そのあとコードを検査させ（`node --check main.mjs`、または `cargo build --release --target wasm32-unknown-unknown`）、Agentty のプラグインカードで**再起動**を押して反映します。エラーは同じカードの**ログ**にあります。

> [!TIP]
> テンプレートのフォルダの外で作業するエージェント向けにプロンプトを書くなら、[マニフェスト リファレンス](/docs/plugin-manifest)、[UI ツリー](/docs/plugin-protocol)、[プロトコル](/docs/plugin-protocol)を示してください。この 3 ページで全部です。

## 手で書く — Rust

```
hello/
├── agentty-plugin.json   マニフェスト
├── hello.wasm            コンパイル済みモジュール
└── src/lib.rs            そのソース
```

```rust
use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};

#[derive(Default)]
struct Hello {
    clicks: u32,
}

impl Plugin for Hello {
    fn panel_open(&mut self, host: &Host) {
        host.set_panel(ui::column(vec![
            ui::text(format!("Clicked {} times", self.clicks)),
            ui::button("go", "Click me"),
        ]));
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        if event.element == "go" {
            self.clicks += 1;
            self.panel_open(host);
        }
    }
}

export_plugin!(Hello);
```

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "runtime": "wasm",
  "main": "hello.wasm",
  "contributes": { "panel": { "title": "Hello", "surface": "sidebar" } }
}
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

SDK の全体は [Rust と WebAssembly](/docs/plugin-rust) にあります。

## 手で書く — JavaScript

```
~/.agentty/plugins/hello/
├── agentty-plugin.json   マニフェスト
├── main.mjs              プラグインのプログラム
└── agentty-plugin.mjs    SDK
```

SDK は依存のないファイル 1 つです。プラグインページで**開発者ガイド**を押すと、Agentty が型とガイドごと `~/.agentty/plugins/.sdk/` に展開します。そこから `agentty-plugin.mjs` を `main.mjs` の隣にコピーしてください。

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [{ "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }]
  }
}
```

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();

plugin
  .onPanelOpen((context) =>
    plugin.setPanel(
      ui.column([
        ui.text('Hello', 'title'),
        ui.text(context.pane ? `You are in ${context.pane.cwd}` : 'No terminal focused', 'muted'),
        ui.button('explain', 'Explain this folder', { icon: 'bot', variant: 'primary' }),
      ]),
    ),
  )
  .onEvent('explain', (_event, context) => explain(context))
  .command('hello.explain', ({ context }) => explain(context))
  .start();

function explain(context) {
  return plugin.injectPrompt({ text: 'Give me a short tour of this project.', cwd: context.pane?.cwd, target: 'ask' });
}
```

> [!WARNING]
> stdout には絶対に書かないでください（`console.log`）。プロトコルが通る経路です。ログは `plugin.log()` か `console.error()` で残します。

残りは [Node.js SDK](/docs/plugin-sdk) にあります。

## インストールする

**プラグイン → フォルダからインストール…** でフォルダを選ぶか、**開発用フォルダをリンク…** でその場所のまま動かします。そのあと**再読み込み**すれば（あるいはページを開き直せば）プラグインがインストール済みとして現れ、パネルのボタンは `surface` が指す場所に、コマンドはパレット（⇧⌘P）に現れます。

## テストする

`node`・`python`・`executable` のプラグインは stdin を読んで stdout に書くプログラムなので、Agentty なしで動かせます。

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"hello","dataDir":"/tmp"},"context":{}}}' \
  '{"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}' \
  | node main.mjs
```

`initialize` に応答したあと、`ui/setPanel` リクエストを送れば正常です。

## 次に

- [マニフェスト リファレンス](/docs/plugin-manifest) — すべてのフィールド
- [Rust と WebAssembly](/docs/plugin-rust) · [Node.js SDK](/docs/plugin-sdk)
- [AgentOS プラグイン](/docs/plugin-agentos) — エージェントを通して仕事を進める
- [権限](/docs/plugin-permissions) · [配布する](/docs/plugin-publishing)
