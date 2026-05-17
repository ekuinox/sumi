# Agent Instructions

Claude Code / Codex CLI 共通の作業ガイドです。コーディングエージェントは作業を始める前にここを一読してください。

## What This Project Is

Windows 専用のリアルタイム音声ビジュアライザです。

- 任意の WASAPI 入力デバイスから音声を取得
- FFT で周波数スペクトラム + 時間領域波形を取り出す
- それらを wgpu + WGSL シェーダーに渡して可視化
- シェーダーファイルは `assets/` に置き、`--shader <path>` で起動時に選択、保存すると即ホットリロード

## Project Layout

- `src/main.rs` — `winit 0.30` の ApplicationHandler。capture スレッドを立てて毎フレーム音声を取り、FFT 整形 → wgpu に渡して描画。
- `src/audio.rs` — WASAPI 共有モード capture を別スレッドで回し、リングバッファに mono サンプルを溜める。デバイスは friendly name の部分一致で選択。
- `src/dsp.rs` — Hann + FFT (`spectrum-analyzer`) + log バンド集約 + dB スケール。シェーダーへは `[f32; N_BARS]` で渡す。
- `src/render.rs` — wgpu (winit 0.30) のセットアップ。`Renderer::reload_shader` で WGSL のホットリロード、バリデーションエラーは旧パイプラインを維持して継続。
- `assets/*.wgsl` — フラグメントシェーダー群。binding layout は以下:
  - group(0) binding(0): `Bars` (storage, FFT バー値の配列)
  - group(0) binding(1): `Globals` (uniform, resolution / time)
  - group(0) binding(2): `wave` (storage, 平滑化済み時間領域波形)
- `examples/*.rs` — 音声経路の検証用。本体ビルドとは独立に動く。

## Audio Capture (重要)

汎用的に「WASAPI 入力デバイスから capture」する設計。デバイス名は `--device <部分一致>` で指定できます (デフォルト値は CLI 定義参照)。

注意: Windows のユーザー空間 API では、特定アプリが排他モードで掴んだ音を普通の loopback で取得できません。アプリ単位 capture (Process Loopback Capture) も WASAPI 排他モードには未対応です。アプリが排他モードで再生してる音を可視化したい場合は、オーディオインターフェース側のハードウェアループバック機能を有効にして、その仮想入力デバイスを capture してください。

## Running

```powershell
just run                              # デフォルトシェーダーで起動
just run <shader-shortcut>            # Justfile に登録された各シェーダー
cargo run -- --shader <path>          # 任意のシェーダーパス
cargo run -- --device <name>          # 任意の入力デバイス
cargo run --example <name>            # 検証用 example
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
