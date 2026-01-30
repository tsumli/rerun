# Video Export Feature

Export Rerun viewer recordings as MP4 video files.

## Requirements

- FFmpeg installed and in PATH
- Build with `video_export` feature

## Quick Start

```bash
# Build
cargo build --release -p rerun-cli --features video_export --no-default-features

# Basic export
./target/release/rerun recording.rrd --video-export output.mp4

# With blueprint (applies show_labels, view settings, etc.)
./target/release/rerun recording.rrd blueprint.rbl --video-export output.mp4

# With options
./target/release/rerun recording.rrd blueprint.rbl \
  --video-export output.mp4 \
  --window-size 1920x1080 \
  --video-fps 30 \
  --video-duration 10 \
  --video-speed 2.0
```

## CLI Options

| Option | Default | Description |
|--------|---------|-------------|
| `--video-export <PATH>` | - | Output MP4 path |
| `--window-size <WxH>` | 1920x1080 | Resolution |
| `--video-fps <FPS>` | 30 | Frames per second |
| `--video-duration <SECS>` | 0 | Duration (0 = auto-detect) |
| `--video-speed <X>` | 1.0 | Playback speed multiplier |

## Blueprint Support

When a `.rbl` blueprint file is provided alongside the `.rrd` recording:
- View layout and settings are applied
- Component overrides (e.g., `show_labels`) are respected
- `PlaybackSpeed` from blueprint is used if set (overrides `--video-speed`)
