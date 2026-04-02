# FunPedals

Real-time guitar multi-effects processor for Raspberry Pi Zero 2W,
built with Rust, [FunDSP](https://github.com/SamiPerttu/fundsp), ALSA, and SDL2.

## Demo

[![FunPedals Demo](https://img.youtube.com/vi/C3Qr_cxG9GM/0.jpg)](https://www.youtube.com/watch?v=C3Qr_cxG9GM)

## Features

- 20 built-in presets, fully editable via touchscreen GUI or TOML
- Multiple effects can be chained freely (e.g. Overdrive → EQ → Reverb)
- Real-time effects:
  - Overdrive, Distortion
  - AutoWah, Chorus, Flanger, Phaser
  - Echo, Reverb
  - EQ3Band
  - NoiseGate, Compressor, Limiter
  - RingMod, OctaveUp
  - GuitarSynth (autocorrelation-based pitch detection + square wave oscillator)
- Touchscreen GUI with waveform and spectrum display
- Page-tab preset browser (10 presets per page, expandable)
- Parameter sliders with physical units (dB / ms / Hz)
- Preset save/load via TOML (`~/.config/funpedals/presets.toml`)
- Terminal menu for headless / macOS use

## Hardware Setup

```
Guitar
  └─→ DIY preamp (single-transistor, powered by Sound Blaster plug-in power)
        └─→ Sound Blaster Play! 3 (USB audio interface)
              └─→ Raspberry Pi Zero 2W
                    └─→ Sound Blaster Play! 3 (output)
                          └─→ Amplifier / Speaker
```

### DIY Preamp

A simple single-transistor preamplifier circuit powered by the plug-in power
supplied from the Sound Blaster Play! 3 microphone input.
See [`docs/preamp_schematic.png`](docs/preamp_schematic.png) for the schematic.

## Requirements

### Hardware

- Raspberry Pi Zero 2W
- USB audio interface (tested with **Creative Sound Blaster Play! 3**)
- Touchscreen display (800×480, optional — terminal mode also available)
- DIY guitar preamp (or any mic-level guitar signal)

### Software

- Raspberry Pi OS (Debian Trixie, 32-bit Desktop)
- Rust (edition 2024)
- ALSA development libraries
- SDL2 and SDL2_ttf
- Noto Sans font (`fonts-noto`)

```bash
sudo apt install libasound2-dev libsdl2-dev libsdl2-ttf-dev fonts-noto
```

## Building

`Cargo.toml` dependencies:

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

## Usage

### GUI mode (Raspberry Pi with touchscreen)

```bash
DISPLAY= WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/$(id -u) \
  cargo run --release -- --gui
```

### Terminal mode (macOS or headless)

```bash
cargo run --release
```

Terminal commands:

| Input | Action |
|-------|--------|
| `1`–`N` | Select preset by number |
| `P` | Show current parameters |
| `S <name>` | Save current state as preset |
| `R` | Reload presets.toml |

## Presets

Presets are stored in `~/.config/funpedals/presets.toml`.
Default presets are written automatically on first launch.
You can edit the file directly or use the GUI PARAM screen to adjust and save.

## Tips

- Set mic gain: `amixer -c 1 cset name='Mic Capture Volume' 50,50`
- Disable USB autosuspend for stable audio:
  ```bash
  echo 'options usbcore autosuspend=-1' | sudo tee /etc/modprobe.d/usb-autosuspend.conf
  ```
- Reduce GPU memory for more RAM:
  Add `gpu_mem=16` to `/boot/firmware/config.txt`

## Acknowledgements

Special thanks to **[Sami Perttu](https://github.com/SamiPerttu)** for creating
[FunDSP](https://github.com/SamiPerttu/fundsp) — an elegant and powerful audio
DSP library for Rust. FunDSP made it possible to implement expressive,
high-quality effects with remarkably concise code. This project would not exist
in its current form without it.

This project was developed with the assistance of **[Claude](https://claude.ai)**
by Anthropic — from architecture design and DSP implementation to debugging and
code refinement, all through natural conversation.

## License

MIT
