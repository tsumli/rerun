# Video Export Feature

Export Rerun viewer recordings as MP4 video files using FFmpeg.

## Requirements

- FFmpeg must be installed and available in your PATH
- Build with the `video_export` feature flag

## Installation

### FFmpeg Installation

- **macOS:** `brew install ffmpeg`
- **Ubuntu/Debian:** `sudo apt install ffmpeg`
- **Fedora:** `sudo dnf install ffmpeg`
- **Arch Linux:** `sudo pacman -S ffmpeg`
- **Windows:** Download from https://ffmpeg.org/download.html

## Usage

```bash
# Build with video_export feature
cargo build -p rerun-cli --no-default-features --features native_viewer,video_export

# Basic usage - export full timeline
cargo run -p rerun-cli --no-default-features --features native_viewer,video_export -- \
  --video-export output.mp4 recording.rrd

# With custom FPS
cargo run -p rerun-cli --no-default-features --features native_viewer,video_export -- \
  --video-export output.mp4 --video-fps 60 recording.rrd

# With specific duration (in seconds)
cargo run -p rerun-cli --no-default-features --features native_viewer,video_export -- \
  --video-export output.mp4 --video-duration 10 recording.rrd

# With playback speed (e.g., 5x faster)
cargo run -p rerun-cli --no-default-features --features native_viewer,video_export -- \
  --video-export output.mp4 --video-speed 5.0 recording.rrd

# Combine options
cargo run -p rerun-cli --no-default-features --features native_viewer,video_export -- \
  --video-export output.mp4 --video-fps 60 --video-speed 2.0 recording.rrd
```

## CLI Options

| Option | Default | Description |
|--------|---------|-------------|
| `--video-export <PATH>` | - | Output path for the MP4 video file |
| `--video-fps <FPS>` | 30 | Frames per second for the output video |
| `--video-duration <SECS>` | 0 | Duration in seconds (0 = full timeline) |
| `--video-speed <MULTIPLIER>` | 1.0 | Playback speed multiplier (e.g., 2.0 for 2x speed) |

## Output Format

- **Resolution:** 1920x1080 (default)
- **Codec:** H.264 (libx264)
- **Pixel Format:** YUV420P
- **Container:** MP4

## Architecture

The video export uses a background thread for FFmpeg encoding:

```
Main thread:    [render frame] -> [send to queue] -> [render next] -> ...
                                        |
Encoder thread:                   [write to FFmpeg] -> [encode] -> ...
```

This allows rendering to continue while encoding happens in parallel, improving performance.

## Timeline Detection

When `--video-duration 0` (default), the exporter automatically detects the timeline duration:

1. Prefers time-based timelines (`DurationNs`, `TimestampNs`) over sequence timelines
2. Uses the timeline with the longest duration
3. Falls back to 10 seconds if no timeline is detected

## Troubleshooting

### FFmpeg not found

Ensure FFmpeg is installed and in your PATH:
```bash
ffmpeg -version
```

### Video appears static

Make sure your recording has time-varying data. The exporter advances through the timeline automatically.

### Export is slow

Video export renders each frame headlessly, which takes time. For faster exports:
- Use `--video-speed` to cover more timeline in less video time
- Reduce resolution (currently hardcoded to 1920x1080)
- Use a recording with less complex visualizations

## Implementation Details

- **Source:** `crates/viewer/re_viewer/src/video_exporter.rs`
- **Feature flag:** `video_export` in `re_viewer`, `rerun`, and `rerun-cli`
- **Dependencies:** `egui_kittest` for headless rendering, `indicatif` for progress bars
