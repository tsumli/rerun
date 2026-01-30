//! Video export functionality using FFmpeg.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Number of frames to buffer between render thread and encoder thread.
/// Larger values provide more buffering but use more memory.
/// At 1920x1080 RGBA, each frame is ~8MB, so 8 frames = ~64MB buffer.
const FRAME_QUEUE_SIZE: usize = 8;

use egui_kittest::Harness;
use indicatif::{ProgressBar, ProgressStyle};
use re_build_info::build_info;
use re_chunk_store::LatestAtQuery;
use re_sdk_types::blueprint::archetypes::TimePanelBlueprint;
use re_viewer_context::{TimeControlCommand, blueprint_timeline, time_panel_blueprint_entity_path};

use crate::{
    App, AppEnvironment, AsyncRuntimeHandle, MainThreadToken, StartupOptions,
    customize_eframe_and_setup_renderer,
};

/// Errors that can occur during video export.
#[derive(thiserror::Error, Debug)]
pub enum VideoExportError {
    #[error("FFmpeg not found. Please install FFmpeg and ensure it's in your PATH.\n\nInstallation instructions:\n  - macOS: brew install ffmpeg\n  - Ubuntu/Debian: sudo apt install ffmpeg\n  - Fedora: sudo dnf install ffmpeg\n  - Arch Linux: sudo pacman -S ffmpeg\n  - Windows: Download from https://ffmpeg.org/download.html")]
    FfmpegNotFound,

    #[error("Failed to start FFmpeg: {0}")]
    FfmpegStartFailed(std::io::Error),

    #[error("Failed to write frame to FFmpeg (broken pipe). FFmpeg may have crashed or exited early: {0}")]
    WriteFrameFailed(std::io::Error),

    #[error("FFmpeg encoding failed:\n{0}")]
    FfmpegFailed(String),

    #[error("Invalid resolution '{0}'. Expected format: WIDTHxHEIGHT (e.g., 1920x1080)")]
    InvalidResolution(String),

    #[error("Render failed: {0}")]
    RenderFailed(String),

    #[error("No recording data loaded. Please provide at least one valid .rrd file.")]
    NoDataLoaded,

    #[error("Encoder thread panicked")]
    EncoderThreadPanicked,

    #[error("Failed to send frame to encoder thread")]
    FrameSendFailed,
}

/// Configuration for video export.
#[derive(Debug, Clone)]
pub struct VideoExportConfig {
    pub output_path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration_secs: f32,
    /// Playback speed multiplier (e.g., 2.0 for 2x speed).
    pub speed: f32,
}

impl Default for VideoExportConfig {
    fn default() -> Self {
        Self {
            output_path: std::path::PathBuf::from("output.mp4"),
            width: 1920,
            height: 1080,
            fps: 30,
            duration_secs: 10.0,
            speed: 1.0,
        }
    }
}

/// Manages the FFmpeg encoding process with background thread encoding.
///
/// Frames are sent to a background thread via a bounded channel, allowing
/// the main thread to continue rendering while encoding happens in parallel.
pub struct FfmpegEncoder {
    /// Channel sender for frame data.
    sender: Option<SyncSender<Vec<u8>>>,
    /// Handle to the encoder thread.
    encoder_thread: Option<JoinHandle<Result<(), VideoExportError>>>,
}

impl FfmpegEncoder {
    /// Start a new FFmpeg encoding process with background thread.
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

        // Create bounded channel for frame data
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(FRAME_QUEUE_SIZE);

        // Spawn encoder thread
        let encoder_thread = thread::spawn(move || Self::encoder_thread_fn(receiver, child));

        Ok(Self {
            sender: Some(sender),
            encoder_thread: Some(encoder_thread),
        })
    }

    /// Background thread function that receives frames and writes to FFmpeg.
    fn encoder_thread_fn(
        receiver: mpsc::Receiver<Vec<u8>>,
        mut child: Child,
    ) -> Result<(), VideoExportError> {
        // Get stdin handle
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| VideoExportError::FfmpegFailed("FFmpeg stdin not available".into()))?;

        // Process frames until channel is closed
        while let Ok(rgba_data) = receiver.recv() {
            stdin
                .write_all(&rgba_data)
                .map_err(VideoExportError::WriteFrameFailed)?;
        }

        // Close stdin to signal end of input
        drop(child.stdin.take());

        // Wait for FFmpeg to complete
        let output = child
            .wait_with_output()
            .map_err(|e| VideoExportError::FfmpegFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VideoExportError::FfmpegFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Send a frame to the encoder thread.
    ///
    /// This will block if the frame queue is full (backpressure).
    pub fn write_frame(&self, rgba_data: Vec<u8>) -> Result<(), VideoExportError> {
        if let Some(sender) = &self.sender {
            sender
                .send(rgba_data)
                .map_err(|_| VideoExportError::FrameSendFailed)?;
        }
        Ok(())
    }

    /// Finish encoding and wait for the encoder thread to complete.
    pub fn finish(mut self) -> Result<(), VideoExportError> {
        // Drop sender to signal end of frames
        self.sender.take();

        // Wait for encoder thread to complete
        if let Some(thread) = self.encoder_thread.take() {
            thread
                .join()
                .map_err(|_| VideoExportError::EncoderThreadPanicked)??;
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

/// Export the viewer to a video file.
///
/// This function:
/// 1. Creates a headless viewer harness
/// 2. Loads the specified RRD file(s)
/// 3. Iterates through the timeline at the specified FPS
/// 4. Captures each frame and pipes it to FFmpeg
pub fn export_video(
    _main_thread_token: MainThreadToken,
    rrd_paths: Vec<String>,
    config: VideoExportConfig,
) -> Result<(), VideoExportError> {
    re_log::info!(
        "Starting video export to {:?} at {}x{} @ {}fps ({}x speed)",
        config.output_path,
        config.width,
        config.height,
        config.fps,
        config.speed
    );

    let window_size = egui::vec2(config.width as f32, config.height as f32);

    // Create harness with rendering support
    let harness_builder =
        re_ui::testing::new_harness(re_ui::testing::TestOptions::Rendering3D, window_size);

    let rrd_paths_clone = rrd_paths.clone();
    let mut harness: Harness<'static, App> = harness_builder.build_eframe(|cc| {
        cc.egui_ctx.set_os(egui::os::OperatingSystem::Nix);
        customize_eframe_and_setup_renderer(cc).expect("Failed to customize eframe");

        let app = App::new(
            MainThreadToken::i_promise_i_am_only_using_this_for_a_test(),
            build_info!(),
            AppEnvironment::Custom("VideoExport".to_owned()),
            StartupOptions {
                hide_welcome_screen: true,
                memory_limit: re_memory::MemoryLimit::UNLIMITED,
                ..Default::default()
            },
            cc,
            Some(re_redap_client::ConnectionRegistry::new_without_stored_credentials()),
            AsyncRuntimeHandle::from_current_tokio_runtime_or_wasmbindgen()
                .expect("Failed to create AsyncRuntimeHandle"),
        );

        // Open the RRD files
        for path in &rrd_paths_clone {
            app.open_url_or_file(path);
        }

        app
    });

    // Progress bar for loading
    let loading_progress = ProgressBar::new_spinner();
    loading_progress.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("Invalid progress style"),
    );
    loading_progress.set_message("Loading data…");
    loading_progress.enable_steady_tick(Duration::from_millis(100));

    // Let the app initialize and load data
    for _ in 0..60 {
        harness.step();
    }
    loading_progress.finish_with_message("Data loaded");

    // Verify that data was actually loaded
    if harness.state().recording_db().is_none() {
        return Err(VideoExportError::NoDataLoaded);
    }

    // Try to activate any loaded blueprint for the recording's application
    {
        let app = harness.state_mut();
        if let Some(store_hub) = &mut app.store_hub {
            if let Some(recording) = store_hub.active_recording() {
                let app_id = Some(recording.application_id().clone());
                if let Some(app_id) = app_id {
                    // Check if there's a default blueprint for this app
                    if let Some(blueprint_id) = store_hub.default_blueprint_id_for_app(&app_id) {
                        let blueprint_id = blueprint_id.clone();
                        re_log::info!("Activating blueprint for app '{app_id}'");
                        if let Err(err) =
                            store_hub.set_cloned_blueprint_active_for_app(&blueprint_id)
                        {
                            re_log::warn!("Failed to activate blueprint: {err}");
                        }
                    } else {
                        re_log::debug!("No default blueprint found for app '{app_id}'");
                    }
                }
            }
        }
    }

    // Run a few more frames to let the blueprint take effect
    for _ in 0..30 {
        harness.step();
    }

    // Try to read playback speed from the blueprint (if available)
    let effective_speed: f32 = {
        let app = harness.state();
        let blueprint_speed = app.store_hub.as_ref().and_then(|store_hub| {
            let blueprint = store_hub.active_blueprint()?;
            let query = LatestAtQuery::latest(blueprint_timeline());
            let (_, speed) = blueprint
                .latest_at_component_quiet::<re_sdk_types::blueprint::components::PlaybackSpeed>(
                    &time_panel_blueprint_entity_path(),
                    &query,
                    TimePanelBlueprint::descriptor_playback_speed().component,
                )?;
            Some(**speed as f32)
        });

        if let Some(bp_speed) = blueprint_speed {
            re_log::info!("Using playback speed from blueprint: {bp_speed}x");
            bp_speed
        } else {
            config.speed
        }
    };

    // Calculate duration - if 0, try to detect from timeline
    let duration_secs: f64 = if config.duration_secs <= 0.0 {
        // Try to get timeline range from the app by iterating over all timelines
        // Prefer time-based timelines (DurationNs, TimestampNs) over sequence timelines
        let detected_duration = harness.state().recording_db().and_then(|db| {
            let mut best_time_duration: Option<(f64, String)> = None;
            let mut best_sequence_duration: Option<(f64, String)> = None;

            for (timeline_name, timeline) in db.timelines() {
                if let Some(range) = db.time_range_for(&timeline_name) {
                    let duration_raw = (range.max().as_f64() - range.min().as_f64()).abs();
                    if duration_raw > 0.0 {
                        // Convert to seconds based on timeline type
                        match timeline.typ() {
                            re_log_types::TimeType::Sequence => {
                                // For sequence timelines, assume 30fps
                                let duration_secs = duration_raw / 30.0;
                                if duration_secs > 0.1
                                    && best_sequence_duration
                                        .as_ref()
                                        .map_or(true, |(best, _)| duration_secs > *best)
                                {
                                    best_sequence_duration =
                                        Some((duration_secs, timeline_name.to_string()));
                                }
                            }
                            re_log_types::TimeType::DurationNs
                            | re_log_types::TimeType::TimestampNs => {
                                // Nanoseconds to seconds
                                let duration_secs = duration_raw / 1_000_000_000.0;
                                if duration_secs > 0.1
                                    && best_time_duration
                                        .as_ref()
                                        .map_or(true, |(best, _)| duration_secs > *best)
                                {
                                    best_time_duration =
                                        Some((duration_secs, timeline_name.to_string()));
                                }
                            }
                        };
                    }
                }
            }

            // Prefer time-based timelines over sequence timelines
            let result = best_time_duration.or(best_sequence_duration);
            if let Some((duration, name)) = result {
                eprintln!("Using timeline '{}' with duration {:.1}s", name, duration);
                Some(duration)
            } else {
                None
            }
        });

        match detected_duration {
            Some(d) => d,
            None => {
                eprintln!("Could not detect timeline duration, using default 10s");
                10.0
            }
        }
    } else {
        config.duration_secs as f64
    };

    // When using speed > 1.0, we cover more timeline time per video second
    // So total_frames stays the same (video duration), but we advance timeline faster
    let total_frames: u64 = (duration_secs / effective_speed as f64 * config.fps as f64).round() as u64;
    let seconds_per_frame = effective_speed as f64 / config.fps as f64;

    // Move to the beginning of the timeline
    {
        let app = harness.state_mut();
        if let Some(rec_db) = app.recording_db() {
            let store_id = rec_db.store_id().clone();
            let histograms = rec_db.timeline_histograms().clone();
            if let Some(time_ctrl) = app.state.time_controls.get_mut(&store_id) {
                let _ = time_ctrl.handle_time_commands(
                    None::<&crate::app_blueprint_ctx::AppBlueprintCtx<'_>>,
                    &histograms,
                    &[TimeControlCommand::MoveBeginning],
                );
            }
        }
    }
    harness.step();

    // Start FFmpeg encoder (runs on background thread)
    let encoder = FfmpegEncoder::new(&config)?;

    eprintln!(
        "Rendering {} frames ({:.1}s at {}fps)…",
        total_frames, duration_secs, config.fps
    );

    // Progress bar for rendering
    let render_progress = ProgressBar::new(total_frames);
    render_progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} frames ({percent}%) | ETA: {eta}")
            .expect("Invalid progress style")
            .progress_chars("=>-"),
    );
    render_progress.enable_steady_tick(Duration::from_millis(100));

    for frame_idx in 0..total_frames {
        // Step the harness to render the frame
        harness.step();

        // Capture the frame using the render method
        let image = harness
            .render()
            .map_err(VideoExportError::RenderFailed)?;

        // Convert image to raw RGBA bytes and send to encoder thread
        let rgba_data = image.into_raw();
        encoder.write_frame(rgba_data)?;

        render_progress.set_position(frame_idx + 1);

        // Advance the timeline by 1/fps seconds
        {
            let app = harness.state_mut();
            if let Some(rec_db) = app.recording_db() {
                let store_id = rec_db.store_id().clone();
                let histograms = rec_db.timeline_histograms().clone();
                if let Some(time_ctrl) = app.state.time_controls.get_mut(&store_id) {
                    let _ = time_ctrl.handle_time_commands(
                        None::<&crate::app_blueprint_ctx::AppBlueprintCtx<'_>>,
                        &histograms,
                        &[TimeControlCommand::MoveBySeconds(seconds_per_frame)],
                    );
                }
            }
        }
    }

    render_progress.finish_with_message("Rendering complete");

    // Progress spinner for encoding finalization
    let encoding_progress = ProgressBar::new_spinner();
    encoding_progress.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("Invalid progress style"),
    );
    encoding_progress.set_message("Finalizing video encoding…");
    encoding_progress.enable_steady_tick(Duration::from_millis(100));

    // Finish encoding
    encoder.finish()?;

    encoding_progress.finish_with_message(format!(
        "✓ Video exported to {}",
        config.output_path.display()
    ));

    Ok(())
}
