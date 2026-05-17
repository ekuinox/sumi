# Agent Instructions

Claude Code / Codex CLI 共通の作業ガイドです。コーディングエージェントは作業を始める前にここを一読してください。

## Start Here

- このプロジェクトは Windows 専用の音声ビジュアライザです。WASAPI capture + wgpu 描画 + WGSL シェーダーで動きます。
- 詳しい開発ルールは `CONTRIBUTING.md` を参照。
- 実装方針が変わる場合は GitHub Issues / Pull Requests で残す。

## Project Layout

- `src/main.rs` — `winit 0.30` の ApplicationHandler。capture スレッドを立てて毎フレーム FFT → 波形整形 → wgpu に渡して描画。
- `src/audio.rs` — WASAPI 共有モード capture を別スレッドで回し、リングバッファに mono サンプルを溜める。
- `src/dsp.rs` — Hann + FFT (`spectrum-analyzer`) + log バンド集約 + dB スケール。
- `src/render.rs` — wgpu (winit 0.30) のセットアップ。`Renderer::reload_shader` で WGSL のホットリロード。
- `assets/*.wgsl` — フラグメントシェーダー。`--shader <path>` で起動時に選択、保存すると即反映。
  - `spectrum.wgsl` — 96 本の周波数バー
  - `ncs.wgsl` — リング + 内側パーティクルフィールド
  - `proximity.wgsl` — 波形ベースの 3 色オシロリング (時間で円周方向に流れる)
  - `proximity_still.wgsl` — スペクトラム駆動の 3 色リング (回らない、bass/mid/treble 別)
- `examples/*.rs` — 音声経路の検証用。本体ビルドとは独立に動く。
  - `minifuse_loopback.rs` — 採用した経路 (`Mix 3/4` 入力 capture) の最小版
  - `spotify_capture.rs` — Process Loopback Capture の検証 (排他モード非対応で却下)
  - `record.rs` — デバイス全体 loopback (古い)

## Audio Path (重要)

`Spotify → 排他モード → MiniFuse 1 → ハードウェア loopback (Mix 3/4) → WASAPI 入力 capture → chryth` という経路で取得します。Windows のユーザー空間 API では排他モードの音は普通の loopback で取れないため、オーディオインターフェース側のループバック機能を使うのが必須前提です。詳細は git log の検証コミット参照。

`Process Loopback Capture` 経路は仕様上 WASAPI 排他モードに非対応なので、本流には採用していません。アプリ単位 capture が欲しい時の参考として残してあるだけです。

## Running

```powershell
just run                              # spectrum.wgsl で起動
just run-ncs                          # ncs.wgsl
just run-proximity                    # proximity.wgsl (流れる)
just run-proximity-still              # proximity_still.wgsl (回らない)
cargo run -- --shader <path>          # 任意のシェーダー
cargo run --example minifuse_loopback # capture の生存確認
```

## Checks

コミット前に最低限以下を通します。

```bash
just fmt
just lint
just test
```

まとめて実行は `just validate`。

## Commits

コミットメッセージには必ず `Co-authored-by` trailer を入れます。使ってるエージェントに合わせて値を変えてください。

```text
Co-authored-by: Codex <codex@openai.com>
Co-authored-by: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

WIP コミットも歓迎ですが、ビルドが通らない状態のコミットは避ける。
