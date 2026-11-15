---
title: agentty-gpui
description: Agentty 専用の UI フレームワーク。GPU で描画する Rust の UI で、macOS・Windows・Linux に対応し、20 個のクレートとして crates.io で公開しています。
---

agentty-gpui は Agentty 専用の UI フレームワークです。Zed の gpui をベースに作りました。

Agentty の画面を GPU で描くのに必要なものをすべて備えています。エレメントとレイアウト、ウィンドウとメニュー、キーボード・マウス入力、テキストシェーピングとフォント、画像と SVG、そして macOS・Windows・Linux のバックエンドとレンダラーです。Agentty に必要な修正や機能はこのリポジトリに直接入れ、ほかの Rust アプリも同じように使えるよう、20 個のクレートとして [crates.io](https://crates.io/crates/agentty-gpui) で公開しています。

- リポジトリ:[github.com/empty-user77/agentty-gpui](https://github.com/empty-user77/agentty-gpui)
- クレート:[crates.io/crates/agentty-gpui](https://crates.io/crates/agentty-gpui)
- API ドキュメント:[docs.rs/agentty-gpui](https://docs.rs/agentty-gpui)
- ライセンス:Apache License 2.0
- Rust:2024 エディション、ツールチェーン 1.98(Agentty と同じ)

## はじめに

`Cargo.toml` に 2 つのクレートを追加します。フレームワーク本体の `agentty-gpui` と、動作中の OS のバックエンドを起動する `agentty-gpui-platform` です。ライブラリ名は `gpui` と `gpui_platform` なので、コードでは `use gpui::...` と書きます。

```toml
[dependencies]
gpui = { package = "agentty-gpui", version = "0.1" }
gpui_platform = { package = "agentty-gpui-platform", version = "0.1" }
```

```rust
fn main() {
    gpui_platform::application().run(|cx: &mut gpui::App| {
        // ウィンドウを開く、キーを割り当てる など
    });
}
```

プラットフォームごとに有効にするものがあります。

| プラットフォーム | 設定 |
|---|---|
| macOS | `agentty-gpui-platform` の `runtime_shaders` を有効にすると Metal シェーダーを起動時にコンパイルするので、Xcode の Metal ツールチェーンなしでビルドできます。無効にするとビルド時にコンパイルするため、ツールチェーンが必要です(`xcodebuild -downloadComponent MetalToolchain`)。 |
| Windows | 追加の設定はありません。既定の `windows-manifest` 機能がアプリのマニフェストを実行ファイルに埋め込みます。 |
| Linux | 両方のクレートで `wayland` と `x11` の一方または両方を有効にします。 |

## 機能フラグ

`agentty-gpui` の機能です。既定は `font-kit`、`wayland`、`x11`、`windows-manifest` です。

| 機能 | 内容 |
|---|---|
| `font-kit` | システムフォントを探して読み込みます(既定) |
| `wayland`、`x11` | Linux のディスプレイサーバー対応(既定) |
| `windows-manifest` | Windows のアプリマニフェストを実行ファイルに埋め込みます(既定) |
| `screen-capture` | 画面キャプチャ |
| `inspector` | 実行中の UI のエレメントを調べるインスペクター |
| `test-support` | テスト用プラットフォームとヘルパー |
| `leak-detection` | エンティティハンドルを追跡し、テストでリークを検出します。ビルド時に `GPUI_LEAK_DETECTION` を設定しても有効になります |
| `bench-support` | ベンチマークとプロファイラー(`bench` は同じ機能の旧名) |
| `profiler` | フレーム時間のヒストグラム |
| `stacker` | 深い再帰でスタックを伸ばします |
| `hot-reload` | 開発中のコードのホットリロード |

`agentty-gpui-platform` は同じ名前の機能を各バックエンドに渡します(`font-kit`、`inspector`、`screen-capture`、`test-support`、`bench-support`、`wayland`、`x11`、`runtime_shaders`)。既定は `multithreaded` です。

## クレート

すべて同じバージョンで公開され、ビルドではターゲットのクレートだけがコンパイルされます。リンク先は各クレートの crates.io のページです。

### フレームワーク

| クレート | ライブラリ名 | 内容 | 依存 |
|---|---|---|---|
| [agentty-gpui](https://crates.io/crates/agentty-gpui) | `gpui` | フレームワーク本体。エレメント、flexbox レイアウト(Taffy)、ウィンドウ、入力、アクションとキーバインド、テキスト、画像・SVG、非同期エグゼキューター | collections、http-client、macros、refineable、scheduler、shared-string、sum-tree、util、bench-metrics(任意) |
| [agentty-gpui-platform](https://crates.io/crates/agentty-gpui-platform) | `gpui_platform` | 現在の OS のバックエンドを選んでアプリを起動します(`gpui_platform::application()`) | agentty-gpui、macos、windows、linux、wgpu(任意) |
| [agentty-gpui-macros](https://crates.io/crates/agentty-gpui-macros) | `gpui_macros` | フレームワークの derive・属性マクロ | — |

### プラットフォームバックエンドとレンダラー

| クレート | ライブラリ名 | 内容 | 依存 |
|---|---|---|---|
| [agentty-gpui-macos](https://crates.io/crates/agentty-gpui-macos) | `gpui_macos` | macOS バックエンド。AppKit のウィンドウとメニュー、入力、通知、画面キャプチャ(任意) | agentty-gpui、apple、collections、scheduler、util |
| [agentty-gpui-apple](https://crates.io/crates/agentty-gpui-apple) | `gpui_apple` | Apple プラットフォーム共通の Metal レンダラーと Core Text のテキスト | agentty-gpui、collections、util |
| [agentty-gpui-windows](https://crates.io/crates/agentty-gpui-windows) | `gpui_windows` | Windows バックエンド。Direct3D 11 レンダラー、DirectWrite のテキスト、DirectComposition | agentty-gpui、collections、scheduler、util |
| [agentty-gpui-linux](https://crates.io/crates/agentty-gpui-linux) | `gpui_linux` | Linux バックエンド。Wayland と X11、キーボード配列(xkbcommon)、デスクトップポータル | agentty-gpui、collections、http-client、util、wgpu(任意) |
| [agentty-gpui-wgpu](https://crates.io/crates/agentty-gpui-wgpu) | `gpui_wgpu` | Linux で使う wgpu レンダラー | agentty-gpui、collections、util |

### 基盤ライブラリ

| クレート | ライブラリ名 | 内容 | 依存 |
|---|---|---|---|
| [agentty-gpui-util](https://crates.io/crates/agentty-gpui-util) | `gpui_util` | 小さな共通ヘルパー | — |
| [agentty-gpui-shared-string](https://crates.io/crates/agentty-gpui-shared-string) | `gpui_shared_string` | コピーせずに共有する文字列 `SharedString` | — |
| [agentty-gpui-collections](https://crates.io/crates/agentty-gpui-collections) | `collections` | ハッシュマップとハッシュセットの型 | util |
| [agentty-gpui-sum-tree](https://crates.io/crates/agentty-gpui-sum-tree) | `sum_tree` | リストとテキストの土台になる、並行処理に強い B 木 | ztracing |
| [agentty-gpui-scheduler](https://crates.io/crates/agentty-gpui-scheduler) | `scheduler` | 非同期タスクのスケジューラー | — |
| [agentty-gpui-refineable](https://crates.io/crates/agentty-gpui-refineable) | `refineable` | スタイルを部分的に上書きできる `Refineable` トレイト | derive-refineable |
| [agentty-gpui-derive-refineable](https://crates.io/crates/agentty-gpui-derive-refineable) | `derive_refineable` | その derive マクロ | — |
| [agentty-gpui-http-client](https://crates.io/crates/agentty-gpui-http-client) | `http_client` | HTTP クライアントのインターフェース。URL から画像を読み込むときに使います | — |
| [agentty-gpui-zlog](https://crates.io/crates/agentty-gpui-zlog) | `zlog` | ロギング | collections |
| [agentty-gpui-ztracing](https://crates.io/crates/agentty-gpui-ztracing) | `ztracing` | トレーシング。Tracy プロファイラーとの連携(任意) | ztracing-macro、zlog(任意) |
| [agentty-gpui-ztracing-macro](https://crates.io/crates/agentty-gpui-ztracing-macro) | `ztracing_macro` | その属性マクロ | — |
| [agentty-gpui-bench-metrics](https://crates.io/crates/agentty-gpui-bench-metrics) | `bench_metrics` | パフォーマンスカウンター | — |

iOS と Web のバックエンドは含めていません。Agentty は macOS、Windows、Linux 向けだからです。元の HTTP クライアントが必要な `image` と `image_gallery` のサンプルも外しています。

## crates.io の依存関係

crates.io のクレートは crates.io のクレートにしか依存できません。元のコードが Git やフォークから取る依存関係は、crates.io のリリースに置き換えています。

| クレート | ここで使うバージョン | 元の側にだけあるもの |
|---|---|---|
| `async-task` | 4.7.1 | タスクの waker vtable の同一性の修正 |
| `calloop`(Linux) | 0.14.5 | ループごとの idle コールバックを約 10ms に制限 |
| `khronos-egl`(Linux) | 6.0.0 | 失敗した EGL 呼び出しが成功を返したときにパニックしない |
| `wgpu`、`naga`(Linux) | 30.0.2 | より新しい API(`QueueDescriptor`、`has_srgb_suffix`) |
| `zed-font-kit`、`zed-scap`、`zed-xim` | crates.io のリリース | その後の小さな修正 |
| `oo7`(Linux) | 0.6 | プロンプトストリームが終わったときのエラー |
| `windows-capture`(Windows、`screen-capture`) | 1.4.4、`Cargo.lock` で固定 | なし。1.5 は `zed-scap` とビルドできないため、`screen-capture` を有効にする側も 1.4.4 に固定する必要があります |

## 元のコードから変更したファイル

変更したファイルには、クレートのマニフェストも含め、すべて先頭に `Modified for Agentty` コメントがあります。

- `crates/gpui/src/style.rs`:テストが `#[perf]` の代わりに `#[test]` を使います。
- `crates/gpui_apple/build.rs`:`../gpui` がないとき(公開されたクレート)、同梱の `gpui-src/` のコピーを読みます。スクリプトがこのコピーを `crates/gpui/src` と同じに保ちます。
- `crates/gpui_wgpu/src/{wgpu_context,wgpu_renderer,wgpu_atlas}.rs`:wgpu 30.0.2 の API に合わせています。

クレートのマニフェストは `scripts/apply_agentty.py` が書きます。パッケージ名を `agentty-gpui-*` にし、ワークスペースのバージョンを入れ、iOS と Web を外し、Git の依存関係を crates.io に置き換えます。このスクリプトの結果がコミットされていないと CI は失敗します。

## 更新の仕方

ブランチは 2 つです。

- **`main`**:公開されるコード。Agentty のための変更はすべてここにあります。
- **`upstream`**:ベースにした元のコードをそのまま置いたブランチ。元のリリースごとに 1 コミットずつ積み、どのリリースかは `zed/SNAPSHOT.md` に書いてあります。

新しい元のリリースを取り込む手順です。

```sh
scripts/sync-upstream.sh v1.29.0   # upstream に新しいスナップショット
git merge upstream                 # main にマージ。競合はここで変えたファイルだけ
python3 -I scripts/apply_agentty.py
```

マージ中にクレートのマニフェストが競合したら、元の側を取り(`git checkout --theirs <ファイル>`)、スクリプトを実行し直します。

## テスト

```sh
cargo nextest run --workspace --features agentty-gpui-apple/runtime_shaders
cargo test --workspace --doc --features agentty-gpui-apple/runtime_shaders
```

- nextest を使います。いくつかのテストはプロセス全体の状態を読むので、`cargo test` のように並べて実行すると失敗します。
- CI は `--profile ci`(`.config/nextest.toml`)で、失敗したテストを 2 回まで再実行します。80ms のポーリングを計るベンチテストが、混んだランナーで間に合わないことがあるためです。
- Linux の CI は、wgpu レンダラーのテストに Mesa のソフトウェア Vulkan ドライバーを使います。

## 公開の手順

1. `scripts/apply_agentty.py` の `VERSION` を上げ、スクリプトを実行し直してプッシュします。`cargo publish --workspace --dry-run` を含め、CI が通る必要があります。
2. GitHub Actions の **Agentty** ワークフローを実行します。このチェックアウトで Agentty を macOS、Windows、Linux でビルドしてテストします。
3. `cargo publish --workspace` で依存順に公開します。
4. リリースのタグを付けます(`git tag v0.1.0 && git push origin v0.1.0`)。

## 問題の報告

agentty-gpui のバグや変更の要望は [agentty-gpui の Issues](https://github.com/empty-user77/agentty-gpui/issues) へ、Agentty のバグは [Agentty の Issues](https://github.com/empty-user77/Agentty/issues) へお願いします。OS ごとの Agentty の違いは [プラットフォーム](/docs/platforms) を参照してください。
