---
title: Rust と WebAssembly
description: Rust でプラグインを書き、どこでも動く .wasm ファイル 1 つとして配布し、プロトコルが与えたものだけに触れさせる。
---

`"runtime": "wasm"` を指定すると、`main` は Agentty が**自分の中で**インタプリタとして実行する WebAssembly モジュールになります。1 つのファイルが macOS・Windows・Linux でそのまま動き、利用者のマシンにインストールするものは何もありません。Node.js も Python も不要です。

[マーケットプレイス](/docs/plugin-publishing)が受け付ける唯一の形式でもあります。そうできるのは、モジュールがプロトコルの渡したものにしか触れられないからです。

## モジュールが触れられるもの

Agentty がモジュールに渡す関数は 3 つだけです。

| `agentty` モジュールからの import | |
|---|---|
| `send(ptr, len)` | Agentty へ送る UTF-8 JSON メッセージ 1 件 |
| `log(ptr, len)` | プラグインのログに残す 1 行 |
| `now_ms() -> i64` | Unix エポックからのミリ秒 |

ファイルもソケットも環境変数もプロセスもなく、そのカウンタ以外に時計もありません。WebAssembly プラグインは**どう書かれていても** `~/.agentty` も、あなたのプロジェクトも、資格情報も読めません。読まないと約束しているからではなく、そのための関数を最初から渡されていないからです。これ以外を import するモジュールは読み込まれません。

残りはすべて、プロセス型プラグインが stdout に書くのと同じ JSON-RPC メッセージで Agentty に要求し、`agentty-plugin.json` の[権限](/docs/plugin-permissions)も同じように検査されます。

> [!NOTE]
> `runtime` が `node`・`python`・`executable` のプラグインは正反対です。利用者の権限で動き、利用者が起動する他のプログラムと同じアクセス権を持ちます。プラグインページの**情報 → 実行方式**にどちらかが表示されます。

## SDK

Rust SDK は[マーケットプレイスのリポジトリ](https://github.com/empty-user77/Agentty-Marketplace/tree/main/sdk/rust)の `sdk/rust` にあり、それで書かれたプラグインが並んでいます。

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
agentty-plugin = { path = "../../sdk/rust" }   # またはそのリポジトリへの git 依存

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true
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

## `Host` が提供するもの

| | |
|---|---|
| `set_panel(tree)` · `show_panel()` | `ui::` で組み立てたパネル |
| `notify_user(kind, message)` · `set_badge(text)` | 通知と、アイコン上の最大 8 文字 |
| `copy(text)` | クリップボードにコピー |
| `log(line)` | プラグインページのログ |
| `open_url(url)` | 利用者のブラウザでページを開く |
| `fetch(request)` | HTTP リクエスト — `net.request` が必要 |
| `prompt(text, target)` | エージェントへのプロンプト — `prompt.inject` が必要 |
| `call(method, params)` · `notify(method, params)` | プロトコルのその他すべて |
| `context()` · `now_ms()` | 利用者の現在位置と時計 |

`call` と `fetch` はリクエスト id を返し、応答は `Plugin::answer` に届きます。

モジュールには自分のファイルがないので、実行のあいだに何かを覚える手段は `storage/get`・`storage/set`・`storage/keys` です。自分のフォルダにある JSON ドキュメント 1 つで、キー 64 個・合計 1 MB まで。権限は不要です。

## ビルドとインストール

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

モジュールを `agentty-plugin.json` の隣に置き、**プラグイン → フォルダからインストール…** でそのフォルダを選びます。ビルドし直したら、プラグインページの**再起動**で反映されます。

## ロゴ

モジュールには画像を同梱するファイルのフォルダがないので、モジュール自身が画像を持ちます。WebAssembly のカスタムセクション `agentty.logo` です。エンジンはこのセクションを無視し、マーケットプレイス項目のチェックサムがすでにこの領域を含みます — ロゴもモジュールの他の部分と同じく、レビュー済みのバイトです。Rust では static ひとつです:

```rust
#[used]
#[link_section = "agentty.logo"]
static LOGO: [u8; 1234] = *include_bytes!("logo.png");
```

`#[used]` は省けません。何も参照しない static はリリースビルドで削除され、セクションも一緒に消えます。配列の長さはファイルの長さと一致している必要があります。

バイトは 512KB 以下の PNG・JPEG・GIF・WebP である必要があり、インストール時に確認されます。SVG はどう名付けても拒否されます — [ロゴ](/docs/plugin-manifest#ロゴ)を参照。Agentty はインストール時に画像をファイルへ書き出すので、行を描画するたびにモジュールを解析することはありません。

## 制限

| | |
|---|---|
| モジュールサイズ | 64 MB（マーケットプレイス登録は 8 MB） |
| メモリ | 64 MB |
| メッセージ 1 件 | 16 MB |
| メッセージごとの作業量 | 予算があり、戻ってこないプラグインは「時間内に終わらなかった」として停止 |
| 1 件を処理する間に送ったメッセージ | `send` と `log` の合計が 256 件を超えると停止 |

一度に 1 件ずつ処理します。モジュールはメッセージを処理している**あいだだけ**動くので、バックグラウンドのループはありません。`host/timer` は時間が経つと応答されるリクエストで、プラグインが得られるのはその応答だけです。

## 実例

どちらもマーケットプレイスのリポジトリにソースごとあり、**プラグイン → マーケットプレイス**からインストールできます。

- [`hello-rust`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/hello-rust) — パネルとカウンタ。権限を 1 つも要求しません。
- [`agent-rest-client`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/agent-rest-client) — 環境、保存したリクエスト、プロキシを備えた HTTP クライアント。`net.request` を使います。

## 次に

- [AgentOS プラグイン](/docs/plugin-agentos) — エージェントを通して仕事を進めるプラグイン
- [マニフェスト リファレンス](/docs/plugin-manifest) · [プラグイン プロトコル](/docs/plugin-protocol) — モジュールの ABI を含む
- [権限](/docs/plugin-permissions) · [配布する](/docs/plugin-publishing)
