# Development Rules

## 開発の進め方

- 音声経路や描画の方向性を変える時はコミットメッセージか PR で残す。
- シェーダーは保存即ホットリロードなので、見た目の試行錯誤は実行中のままで OK。
- 大きな実装に入る前にいったんコミットしておくと、気軽にロールバックできる。

## コミット前のチェック

最低限以下を通します。

```bash
just fmt
just lint
just test
```

まとめて実行は:

```bash
just validate
```

`just validate` の内訳:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

コミットメッセージには必ず `Co-authored-by` trailer を付ける。使ってるエージェントに合わせる:

```text
Co-authored-by: Codex <codex@openai.com>
Co-authored-by: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

## ツール管理

- Rust toolchain は `rust-toolchain.toml` で固定 (1.95.0)。
- 標準タスクは `Justfile` で管理。
- 依存追加時はできるだけ最新の安定版を確認する。理由が薄い場合は標準ライブラリや既存依存で足りないか考える。

## Rust コードを書くとき

### コード品質

- Clippy の警告は可能な限り解消する。
- 不要な警告を許可する場合は理由をコメントで明記する。
- `cargo fmt` でフォーマット。

### import の書き方

おおむね以下の順:

- `std`
- 外部クレート
- `crate`
- `super`
- `self`

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use winit::application::ApplicationHandler;

use crate::audio::spawn_capture;
```

トレイトメソッドだけを使う import は `as _` を付ける。

### モジュール内の書き方

- 外部に公開する関数や構造体をファイル上部に置く。
- 公開しない補助関数や補助型は下部。
- `impl` は可能な範囲で型定義の近くに。
- `main()` から近いもの (= レイヤーが上のもの) ほど浅い位置に。

### エラーメッセージ

- log / tracing など開発者向けは英文で書く。
- ユーザーの目に入るメッセージは日本語で書く。

## シェーダーを書くとき

- `assets/*.wgsl` に追加。`--shader <path>` で起動時に選択できる。
- 既存の binding layout (group 0 = bars/globals/wave) を維持。未使用バインディングは `_ = wave[0];` のように phony assignment で参照する。
- 編集 → 保存で実行中のウィンドウに即反映される。シンタックスエラー時は古いパイプラインが残ってログに WGSL のパースエラーが出る。
- 数値パラメータは可能な範囲でファイル冒頭に `let` でまとめておくと触りやすい。

## ドキュメント

- コーディングエージェント向けの作業ルールは `AGENTS.md`。
- 詳細な開発ルールは本ファイル (`CONTRIBUTING.md`)。
- 音声経路や描画方針の決定は git log / コミットメッセージで残す (専用の `requirements.md` は今のところ作らない。必要になれば足す)。
