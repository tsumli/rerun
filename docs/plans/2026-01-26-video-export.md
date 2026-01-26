# Video Export Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `--video-export` flag to export the viewer as an MP4 video using headless rendering.

**Architecture:** Use `egui_kittest::Harness` for headless rendering (already used in tests), iterate through the timeline at fixed FPS, capture frames via egui's screenshot mechanism, and pipe RGBA frames to FFmpeg for H.264 encoding.

**Tech Stack:** Rust, egui_kittest (headless), FFmpeg (external process via stdin pipe), clap (CLI)

---

## Task 1: Add CLI Arguments

**Files:**
- Modify: `crates/top/rerun/src/commands/entrypoint.rs`

**Step 1: Add video export arguments to Args struct**

In `crates/top/rerun/src/commands/entrypoint.rs`, add these fields after `screenshot_to` (around line 150):

```rust
    /// Export the viewer as a video file and quit.
    /// Renders headlessly at the specified FPS through the entire timeline.
    /// Requires FFmpeg to be installed.
    #[clap(long)]
    video_export: Option<std::path::PathBuf>,

    /// Frames per second for video export.
    /// Only used with --video-export.
    #[clap(long, default_value = "30")]
    video_fps: u32,
```

**Step 2: Build and verify arguments are recognized**

Run: `cargo build -p rerun`
Then: `cargo run -p rerun -- --help | grep -A2 video`
Expected: Shows `--video-export` and `--video-fps` in help output

**Step 3: Commit**

```bash
git add crates/top/rerun/src/commands/entrypoint.rs
git commit -m "feat: add --video-export and --video-fps CLI arguments"
```

---

## Task 2: Create VideoExporter Module

**Files:**
- Create: `crates/viewer/re_viewer/src/video_exporter.rs`
- Modify: `crates/viewer/re_viewer/src/lib.rs`

**Step 1: Create the video_exporter module with FFmpeg process management**

Create `crates/viewer/re_viewer/src/video_exporter.rs`:

```rust
//! Video export functionality using FFmpeg.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Errors that can occur during video export.
#[derive(thiserror::Error, Debug)]
pub enum VideoExportError {
    #[error("FFmpeg not found. Please install FFmpeg and ensure it's in your PATH.")]
    FfmpegNotFound,

    #[error("Failed to start FFmpeg: {0}")]
    FfmpegStartFailed(std::io::Error),

    #[error("Failed to write frame to FFmpeg: {0}")]
    WriteFrameFailed(std::io::Error),

    #[error("FFmpeg process failed: {0}")]
    FfmpegFailed(String),

    #[error("Invalid resolution: {0}")]
    InvalidResolution(String),
}

/// Configuration for video export.
#[derive(Debug, Clone)]
pub struct VideoExportConfig {
    pub output_path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for VideoExportConfig {
    fn default() -> Self {
        Self {
            output_path: std::path::PathBuf::from("output.mp4"),
            width: 1920,
            height: 1080,
            fps: 30,
        }
    }
}

/// Manages the FFmpeg encoding process.
pub struct FfmpegEncoder {
    child: Child,
}

impl FfmpegEncoder {
    /// Start a new FFmpeg encoding process.
    pub fn new(config: &VideoExportConfig) -> Result<Self, VideoExportError> {
        // Check if ffmpeg is available
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return Err(VideoExportError::FfmpegNotFound);
        }

        let child = Command::new("ffmpeg")
            .args([
                "-y", // Overwrite output file
                "-f", "rawvideo",
                "-pix_fmt", "rgba",
                "-s", &format!("{}x{}", config.width, config.height),
                "-r", &config.fps.to_string(),
                "-i", "pipe:0", // Read from stdin
                "-c:v", "libx264",
                "-pix_fmt", "yuv420p",
                "-preset", "fast",
                "-crf", "23",
            ])
            .arg(&config.output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(VideoExportError::FfmpegStartFailed)?;

        Ok(Self { child })
    }

    /// Write a single RGBA frame to FFmpeg.
    pub fn write_frame(&mut self, rgba_data: &[u8]) -> Result<(), VideoExportError> {
        if let Some(stdin) = self.child.stdin.as_mut() {
            stdin
                .write_all(rgba_data)
                .map_err(VideoExportError::WriteFrameFailed)?;
        }
        Ok(())
    }

    /// Finish encoding and wait for FFmpeg to complete.
    pub fn finish(mut self) -> Result<(), VideoExportError> {
        // Close stdin to signal end of input
        drop(self.child.stdin.take());

        let output = self
            .child
            .wait_with_output()
            .map_err(|e| VideoExportError::FfmpegFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VideoExportError::FfmpegFailed(stderr.to_string()));
        }

        Ok(())
    }
}

/// Parse resolution string like "1920x1080" into (width, height).
pub fn parse_resolution(s: &str) -> Result<(u32, u32), VideoExportError> {
    let parts: Vec<&str> = s.split('x').collect();
    if parts.len() != 2 {
        return Err(VideoExportError::InvalidResolution(s.to_string()));
    }

    let width = parts[0]
        .parse()
        .map_err(|_| VideoExportError::InvalidResolution(s.to_string()))?;
    let height = parts[1]
        .parse()
        .map_err(|_| VideoExportError::InvalidResolution(s.to_string()))?;

    Ok((width, height))
}
```

**Step 2: Add module to lib.rs**

In `crates/viewer/re_viewer/src/lib.rs`, add after other module declarations:

```rust
#[cfg(not(target_arch = "wasm32"))]
pub mod video_exporter;
#[cfg(not(target_arch = "wasm32"))]
pub use video_exporter::{FfmpegEncoder, VideoExportConfig, VideoExportError};
```

**Step 3: Build and verify**

Run: `cargo build -p re_viewer`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add crates/viewer/re_viewer/src/video_exporter.rs crates/viewer/re_viewer/src/lib.rs
git commit -m "feat: add VideoExporter module with FFmpeg encoding"
```

---

## Task 3: Add StartupOptions for Video Export

**Files:**
- Modify: `crates/viewer/re_viewer/src/startup_options.rs`

**Step 1: Add video export fields to StartupOptions**

In `crates/viewer/re_viewer/src/startup_options.rs`, add after `screenshot_to_path_then_quit` (around line 22):

```rust
    /// Export the viewer as a video file and quit.
    /// Requires FFmpeg to be installed.
    #[cfg(not(target_arch = "wasm32"))]
    pub video_export_path: Option<std::path::PathBuf>,

    /// Frames per second for video export.
    #[cfg(not(target_arch = "wasm32"))]
    pub video_export_fps: u32,
```

**Step 2: Add default values**

In the `Default` impl, add after `screenshot_to_path_then_quit: None,`:

```rust
            #[cfg(not(target_arch = "wasm32"))]
            video_export_path: None,

            #[cfg(not(target_arch = "wasm32"))]
            video_export_fps: 30,
```

**Step 3: Build and verify**

Run: `cargo build -p re_viewer`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add crates/viewer/re_viewer/src/startup_options.rs
git commit -m "feat: add video export options to StartupOptions"
```

---

## Task 4: Wire CLI Args to StartupOptions

**Files:**
- Modify: `crates/top/rerun/src/commands/entrypoint.rs`

**Step 1: Update native_startup_options_from_args**

In `native_startup_options_from_args` function (around line 972), add after `screenshot_to_path_then_quit`:

```rust
        video_export_path: args.video_export.clone(),
        video_export_fps: args.video_fps,
```

**Step 2: Build and verify**

Run: `cargo build -p rerun`
Expected: Compiles without errors

**Step 3: Commit**

```bash
git add crates/top/rerun/src/commands/entrypoint.rs
git commit -m "feat: wire video export CLI args to StartupOptions"
```

---

## Task 5: Create Headless Export Function

**Files:**
- Modify: `crates/viewer/re_viewer/src/video_exporter.rs`
- Modify: `crates/top/rerun/src/commands/entrypoint.rs`

**Step 1: Add export_video function to video_exporter.rs**

Add to the end of `crates/viewer/re_viewer/src/video_exporter.rs`:

```rust
use egui_kittest::Harness;
use re_log_types::TimeInt;

use crate::{App, AppEnvironment, AsyncRuntimeHandle, MainThreadToken, StartupOptions};

/// Export the viewer to a video file.
///
/// This function:
/// 1. Creates a headless viewer harness
/// 2. Loads the specified RRD file
/// 3. Iterates through the timeline at the specified FPS
/// 4. Captures each frame and pipes it to FFmpeg
pub fn export_video(
    main_thread_token: MainThreadToken,
    build_info: re_build_info::BuildInfo,
    rrd_paths: Vec<String>,
    config: VideoExportConfig,
    startup_options: StartupOptions,
) -> Result<(), VideoExportError> {
    re_log::info!(
        "Starting video export to {:?} at {}x{} @ {}fps",
        config.output_path,
        config.width,
        config.height,
        config.fps
    );

    let window_size = egui::vec2(config.width as f32, config.height as f32);

    // Create harness with rendering support
    let mut harness_builder =
        re_ui::testing::new_harness(re_ui::testing::TestOptions::Rendering3D, window_size);

    let mut harness: Harness<'static, App> = harness_builder.build_eframe(|cc| {
        crate::customize_eframe_and_setup_renderer(cc).expect("Failed to customize eframe");

        let mut app = App::new(
            main_thread_token,
            build_info,
            AppEnvironment::Custom("VideoExport".to_owned()),
            StartupOptions {
                hide_welcome_screen: true,
                memory_limit: re_memory::MemoryLimit::UNLIMITED,
                ..startup_options
            },
            cc,
            Some(re_redap_client::ConnectionRegistry::new_without_stored_credentials()),
            AsyncRuntimeHandle::from_current_tokio_runtime_or_wasmbindgen()
                .expect("Failed to create AsyncRuntimeHandle"),
        );

        // Open the RRD files
        for path in &rrd_paths {
            app.open_url_or_file(path);
        }

        app
    });

    // Let the app initialize and load data
    re_log::info!("Loading data…");
    for _ in 0..30 {
        harness.step();
    }

    // Start FFmpeg encoder
    let mut encoder = FfmpegEncoder::new(&config)?;

    // Get timeline range from the app
    // For now, we'll render a fixed number of frames
    // TODO: Get actual timeline range from the loaded data
    let total_frames = 300; // 10 seconds at 30fps as placeholder

    re_log::info!("Rendering {} frames…", total_frames);

    for frame_idx in 0..total_frames {
        // Step the harness to render the frame
        harness.step();

        // Capture the frame
        // Note: This is a simplified version. Full implementation needs
        // to use egui's screenshot functionality and extract pixel data.
        if let Some(image) = harness.try_snapshot() {
            let rgba_data: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
                .collect();
            encoder.write_frame(&rgba_data)?;
        }

        if frame_idx % 30 == 0 {
            re_log::info!("Progress: {}/{} frames", frame_idx, total_frames);
        }

        // TODO: Advance the timeline by 1/fps seconds
    }

    // Finish encoding
    encoder.finish()?;

    re_log::info!("Video export complete: {:?}", config.output_path);
    Ok(())
}
```

**Step 2: Build and verify**

Run: `cargo build -p re_viewer`
Expected: May have some compilation issues to resolve (harness API differences)

**Step 3: Commit**

```bash
git add crates/viewer/re_viewer/src/video_exporter.rs
git commit -m "feat: add export_video function for headless rendering"
```

---

## Task 6: Integrate Video Export into CLI Entry Point

**Files:**
- Modify: `crates/top/rerun/src/commands/entrypoint.rs`

**Step 1: Add video export branch in run_native_viewer_with_file_inputs**

In `run_native_viewer_with_file_inputs` function, add check for video_export before `run_native_app` call:

```rust
    // Check if video export is requested
    if let Some(video_export_path) = &startup_options.video_export_path {
        let config = re_viewer::VideoExportConfig {
            output_path: video_export_path.clone(),
            width: startup_options.resolution_in_points
                .map(|r| r[0] as u32)
                .unwrap_or(1920),
            height: startup_options.resolution_in_points
                .map(|r| r[1] as u32)
                .unwrap_or(1080),
            fps: startup_options.video_export_fps,
        };

        return re_viewer::video_exporter::export_video(
            _main_thread_token,
            _build_info,
            url_or_paths,
            config,
            startup_options,
        )
        .map_err(|e| anyhow::anyhow!("{}", e));
    }
```

**Step 2: Build and verify**

Run: `cargo build -p rerun`
Expected: Compiles without errors

**Step 3: Commit**

```bash
git add crates/top/rerun/src/commands/entrypoint.rs
git commit -m "feat: integrate video export into CLI entry point"
```

---

## Task 7: Manual Integration Test

**Files:** None (manual testing)

**Step 1: Create a test RRD file**

Run: `cargo run -p dna` (or any example that generates data)
This should create some test data or connect to a running rerun.

**Step 2: Test video export**

Run: `cargo run -p rerun -- test.rrd --video-export output.mp4 --video-fps 30`

**Step 3: Verify output**

Check: `ffprobe output.mp4` to verify the video was created correctly

**Step 4: Document any issues found**

If issues are found, create follow-up tasks to address them.

---

## Task 8: Add Basic Error Handling and User Feedback

**Files:**
- Modify: `crates/viewer/re_viewer/src/video_exporter.rs`

**Step 1: Improve error messages for common failure cases**

Update FFmpegNotFound error message with installation instructions:

```rust
    #[error("FFmpeg not found. Please install FFmpeg:\n  - Ubuntu/Debian: sudo apt install ffmpeg\n  - macOS: brew install ffmpeg\n  - Windows: Download from https://ffmpeg.org/download.html")]
    FfmpegNotFound,
```

**Step 2: Add progress reporting**

Add a progress callback parameter to export_video (optional enhancement).

**Step 3: Commit**

```bash
git add crates/viewer/re_viewer/src/video_exporter.rs
git commit -m "feat: improve video export error messages"
```

---

## Notes for Implementation

### Key Files Reference

| Purpose | File |
|---------|------|
| CLI Args | `crates/top/rerun/src/commands/entrypoint.rs` |
| Startup Options | `crates/viewer/re_viewer/src/startup_options.rs` |
| Screenshot Logic | `crates/viewer/re_viewer/src/screenshotter.rs` |
| Test Harness Example | `crates/viewer/re_viewer/src/viewer_test_utils/mod.rs` |
| FFmpeg Integration | `crates/utils/re_video/src/decode/ffmpeg_cli/ffmpeg.rs` |
| Time Control | `crates/viewer/re_viewer_context/src/time_control.rs` |

### Build Commands

- Build viewer: `cargo build -p re_viewer`
- Build CLI: `cargo build -p rerun`
- Run tests: `cargo nextest run --all-features -p re_viewer`
- Format: `pixi run rs-fmt`

### Known Challenges

1. **Timeline iteration**: Need to properly advance the timeline and wait for data to load at each timestamp
2. **Frame capture**: May need to adapt screenshotter.rs logic for headless use
3. **Harness API**: The test harness may have slightly different API than shown - check `viewer_test_utils/mod.rs` for exact usage
