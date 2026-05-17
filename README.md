# sumi

Windows 専用のリアルタイム音声ビジュアライザ。任意の WASAPI 入力デバイスから音を取り、FFT で周波数スペクトラム + 時間領域波形を取り出して wgpu + WGSL シェーダーで描画する。シェーダーは保存即ホットリロードなので、見た目はリポジトリ内 `assets/` を直接いじって試行錯誤できる。

## できること

- 任意の入力デバイスをタスクトレイから選択して可視化
- 4 種類の組み込みシェーダー + 自前シェーダーをファイルダイアログで切替
- WGSL ホットリロード (保存で即反映、バリデーションエラー時は旧パイプライン継続)
- 半透明 + クリック透過の浮動小窓モード (`Always on top`、タスクバーに出さない、操作スルー)
- 4 隅 / 4 辺プリセットへワンクリック配置、複数モニタ対応
- 設定変更で自動再起動 (デバイス / モニタ / シェーダ / 浮動切替)
- 日次ローテーションのファイルログ (件数で自動削除)

## 動かす

### 必要なもの

- Windows 10/11
- DirectX 12 が動く GPU (= 大抵の現行 Windows マシン)
- WASAPI で見える入力デバイス (オーディオ I/F のマイク / line in / ハードウェアループバック等)

### ビルド & 実行

```powershell
just run                     # cargo run と同じ
just config <path>           # 任意の config.toml で起動
cargo run -- --config <path> # 同上 (just を使わない場合)
```

初回起動時に `config.toml` と隣接する `assets/*.wgsl` (binary に埋め込んだ既定シェーダー一式) が自動で書き出される。シェーダーの切替はタスクトレイの **Choose shader...** か、`config.toml` の `shader` フィールドを書き換える。

### デバイスを選ぶ

初回は `device = ""` で起動 (= 無音モード)。タスクトレイの **Audio device** から目的の入力を選ぶと、設定が保存されて自動再起動して以降そのデバイスから取得する。

> **注意**: アプリが WASAPI 排他モードで掴んでる音は通常の loopback では取れない (Process Loopback Capture もアプリ単位だが排他モード非対応)。排他モードで再生してる音を可視化したい場合は、オーディオ I/F 側のハードウェアループバック (例: MiniFuse の `Mix 3/4`) を有効化してその仮想入力を選ぶ。

## タスクトレイメニュー

| 項目 | 動作 |
| --- | --- |
| Show | ウィンドウ表示、最前面化 |
| Minimize | ウィンドウを最小化 (バックグラウンド継続) |
| ● Floating | 浮動小窓モードの ON/OFF (再起動して反映) |
| Move to ▶ | 4 隅 / 4 辺プリセット (浮動モード時の配置)。選択値は config に保存される |
| Monitor ▶ | 表示するモニタを選択 (再起動で反映) |
| Audio device ▶ | 入力デバイスを選択 (再起動で反映) |
| Choose shader... | ファイルダイアログで `*.wgsl` を選んで切替 (再起動で反映) |
| Open config folder | エクスプローラで config.toml を選択状態で開く |
| Restart | プロセスを再起動 |
| Quit | 終了 |

タスクトレイのアイコン自体は 32x32 のミニビジュアライザになっていて、現在のバー値が緑→黄→赤の VU メータで表示される。

## 設定 (`config.toml`)

| キー | 既定 | 説明 |
| --- | --- | --- |
| `device` | `""` | 入力デバイスの friendly name 部分一致 (空 = 無音) |
| `shader` | `assets/spectrum.wgsl` | 描画に使う WGSL ファイルのパス |
| `floating.enabled` | `false` | 起動時に浮動モードで開くか |
| `floating.alpha` | `153` | 浮動モードのウィンドウアルファ (0=透明、255=不透明) |
| `floating.width` | `320` | 浮動モードのウィンドウ幅 (physical px、コーナー配置時) |
| `floating.height` | `64` | 浮動モードのウィンドウ高さ (兼: 辺配置時の「厚み」) |
| `floating.margin` | `0` | 画面端との余白 (physical px) |
| `floating.placement` | `"bottom-right"` | 後述 8 値のいずれか |
| `floating.monitor` | `""` | 表示モニタの Win32 デバイス名 (例: `\\.\DISPLAY2`)。空 = プライマリ |
| `logging.directory` | `""` | ログ出力先 (空 = config と同じディレクトリ下の `logs/`) |
| `logging.retention_days` | `7` | 何件分のログを残すか (0 = 無効化、`tracing-appender` の `max_log_files` に渡る) |

`floating.placement` の有効値: `top-left` / `top-right` / `bottom-left` / `bottom-right` / `top-edge` / `right-edge` / `bottom-edge` / `left-edge`。

`shader` と `logging.directory` は **config.toml の親ディレクトリからの相対パス** でも書ける (絶対パスもそのまま使える)。初回生成は絶対パスで書き出されるが、リポジトリに置いて移動可能にしたい時などは手で相対に書き換えれば OK。タスクトレイの **Choose shader...** で config 配下のシェーダーを選んだ場合は自動で相対パスに変換して保存される。

### config.toml の置き場所

- **release ビルド**: `%APPDATA%\sumi\config.toml`
- **debug ビルド (`cargo run`)**: プロジェクトルートの `./config.toml`
  (隣接する `./assets/*.wgsl` を直接参照するので、ホットリロードで開発フィードバックがそのまま乗る)

`--config <path>` で常に明示指定できる。

## シェーダーを書く

`assets/*.wgsl` にファイルを置いて Choose shader... で選ぶだけ。binding layout は以下:

| binding | 種別 | 内容 |
| --- | --- | --- |
| 0 | storage (RO) | `Bars { data: array<f32, 96> }` — 現フレームの周波数バー値 (0..1 正規化、log バンド) |
| 1 | uniform | `Globals { resolution: vec2<f32>, time: f32, orientation: u32 }` |
| 2 | storage (RO) | `array<f32, 256>` — 平滑化済みの時間領域波形 |
| 3 | storage (RO) | `Bars` — 各 bar の生ピーク値 (Rust 側では減衰させない、`bars[i] >= peaks[i]` で更新) |
| 4 | storage (RO) | `Bars` — 各 bar のピークが最後に更新されてからの経過秒 |

`orientation` は `Move to` で辺配置を選んだとき UV を回す用 (0=normal / 1=90°CW / 2=180° / 3=90°CCW)。

未使用バインディングは `_ = wave[0];` のように phony assignment で 1 度参照しておかないと layout 整合エラーになる。

組み込みシェーダー:

- `spectrum.wgsl` — 虹色バー (定番)
- `spectrum_peakhold.wgsl` — バー + 落ちてくるピーク残光線 (hold / decay / fade はファイル冒頭の定数で調整)
- `ring_field.wgsl` — 黄色リング + 内側ドットフィールド
- `wave_rings.wgsl` — 円周方向に時間領域波形をマップする 3 色オシロリング
- `band_rings.wgsl` — 帯域別 3 本リング (回転なし)

## ファイル構成

```text
src/
  main.rs       # winit ApplicationHandler、トレイ / ウィンドウ / イベントループ
  audio.rs      # WASAPI 共有モード capture + リングバッファ
  dsp.rs        # Hann + FFT + log バンド + dB スケール
  render.rs     # wgpu セットアップ、shader ホットリロード、peak hold state
  floating.rs   # 浮動窓スタイル適用、モニタ列挙、配置計算
  config.rs     # config.toml 読み書き + assets/ scaffold
  logging.rs    # tracing-subscriber + tracing-appender (daily rotation)
  tray.rs       # tray-icon メニュー組み立て + ミニビジュアライザアイコン
assets/
  *.wgsl        # 組み込みシェーダー (binary に埋め込み、初回起動時にユーザー領域へ scaffold)
```

## 開発・コントリビュート

- 作業ガイドは [AGENTS.md](AGENTS.md) (Claude Code / Codex CLI 共通)
- 詳細な開発ルールは [CONTRIBUTING.md](CONTRIBUTING.md)
- `just validate` で fmt / clippy / test を一括チェック
