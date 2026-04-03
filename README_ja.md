# FunPedals

Raspberry Pi Zero 2W 上で動作するリアルタイム・ギター・マルチエフェクトプロセッサです。
Rust、[FunDSP](https://github.com/SamiPerttu/fundsp)、ALSA、SDL2 を使って構築しました。

## デモ

[![FunPedals Demo](https://img.youtube.com/vi/OPpnCckCWFs/0.jpg)](https://www.youtube.com/watch?v=OPpnCckCWFs)

## 特徴

- 20種類のプリセット内蔵（タッチスクリーン GUI または TOML ファイルで編集可能）
- 複数のエフェクトを自由に連結してプリセットを作成可能（例：Overdrive → EQ → Reverb）
- リアルタイムエフェクト：
  - Overdrive、Distortion
  - AutoWah、Chorus、Flanger、Phaser
  - Echo、Reverb
  - EQ3Band
  - NoiseGate、Compressor、Limiter
  - RingMod、OctaveUp
  - GuitarSynth（自己相関法によるピッチ検出 + 矩形波オシレータ）
- 波形・スペクトラム表示付きタッチスクリーン GUI
- ページタブ式プリセットブラウザ（1ページ10件、拡張可能）
- dB / ms / Hz 単位のパラメータスライダー
- TOML によるプリセット保存・読み込み（`~/.config/funpedals/presets.toml`）
- ターミナルメニュー（ヘッドレス環境・macOS 対応）

## ハードウェア構成

```
ギター
  └─→ 自作プリアンプ（トランジスター1石、Sound Blaster のプラグインパワー駆動）
        └─→ Sound Blaster Play! 3（USB オーディオインターフェース）
              └─→ Raspberry Pi Zero 2W
                    └─→ Sound Blaster Play! 3（出力）
                          └─→ アンプ / スピーカー
```

### 自作プリアンプ

Sound Blaster Play! 3 のマイク入力から供給されるプラグインパワーを使用した、
トランジスター1石のシンプルなプリアンプ回路です。
回路図は [`docs/preamp_schematic.png`](docs/preamp_schematic.png) を参照してください。

## 必要環境

### ハードウェア

- Raspberry Pi Zero 2W
- USB オーディオインターフェース（**Creative Sound Blaster Play! 3** で動作確認済み）
- タッチスクリーンディスプレイ（800×480、省略可 — ターミナルモードも使用可能）
- ギター用プリアンプ（自作または マイクレベルの信号が出力できるもの）

### ソフトウェア

- Raspberry Pi OS（Debian Trixie、32ビット Desktop）
- Rust（edition 2024）
- ALSA 開発ライブラリ
- SDL2 および SDL2_ttf
- Noto Sans フォント（`fonts-noto`）

```bash
sudo apt install libasound2-dev libsdl2-dev libsdl2-ttf-dev fonts-noto
```

## ビルド

`Cargo.toml` の依存関係：

```toml
[dependencies]
fundsp = "0.23"
serde = { version = "1", features = ["derive"] }
toml = "0.8"

[target.'cfg(target_os = "linux")'.dependencies]
alsa = "0.9"
ringbuf = "0.4"
sdl2 = { version = "0.36", features = ["ttf"] }

[target.'cfg(target_os = "macos")'.dependencies]
cpal = "0.15"
```

```bash
cargo build --release
```

## 使い方

### GUI モード（タッチスクリーン付き Raspberry Pi）

```bash
DISPLAY= WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/$(id -u) \
  cargo run --release -- --gui
```

### ターミナルモード（macOS またはヘッドレス環境）

```bash
cargo run --release
```

ターミナルコマンド：

| 入力 | 動作 |
|------|------|
| `1`〜`N` | 番号でプリセットを選択 |
| `P` | 現在のパラメータを表示 |
| `S <名前>` | 現在の状態をプリセットとして保存 |
| `R` | presets.toml を再読み込み |

## プリセット

プリセットは `~/.config/funpedals/presets.toml` に保存されます。
初回起動時にデフォルトプリセットが自動的に書き出されます。
ファイルを直接編集するか、GUI の PARAM 画面でパラメータを調整して保存できます。

## Tips

- マイクゲインの設定: `amixer -c 1 cset name='Mic Capture Volume' 50,50`
- USB オートサスペンドを無効化（安定した音声出力のため）：
  ```bash
  echo 'options usbcore autosuspend=-1' | sudo tee /etc/modprobe.d/usb-autosuspend.conf
  ```
- GPU メモリを削減して RAM を確保：
  `/boot/firmware/config.txt` に `gpu_mem=16` を追加

## 謝辞

**[Sami Perttu](https://github.com/SamiPerttu)** 氏が開発した
[FunDSP](https://github.com/SamiPerttu/fundsp) に深く感謝します。
Rust 向けの洗練された強力な音声 DSP ライブラリであり、
FunDSP のおかげで高品質なエフェクトを簡潔なコードで実装することができました。
このプロジェクトは FunDSP なしには現在の形にはなっていません。

本プロジェクトは Anthropic の **[Claude](https://claude.ai)** の支援を受けて開発しました。
アーキテクチャ設計から DSP 実装、デバッグ、コード改善まで、すべて自然な会話を通じて進めました。

## ライセンス

MIT
