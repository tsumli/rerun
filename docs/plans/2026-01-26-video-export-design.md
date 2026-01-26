# Video Export 機能設計

## 概要

ビューワーの画面をヘッドレスでレンダリングし、MP4動画として出力する機能。

## 使用例

```bash
# 基本使用法
rerun recording.rrd --video-export output.mp4

# オプション付き
rerun recording.rrd --video-export output.mp4 --resolution 1920x1080 --fps 30
rerun recording.rrd --video-export output.mp4 --blueprint layout.rbl
```

## 仕様

### 動作モード
- **ヘッドレス（オフスクリーン）**: ウィンドウを表示せずにバックグラウンドでレンダリング

### タイムライン制御
- **固定FPS**: 指定されたFPSでタイムラインを進め、各フレームをレンダリング
- **範囲**: RRDに記録された最初から最後まで自動

### デフォルト値
- 解像度: 1920x1080
- FPS: 30
- コーデック: H.264

### エンコード
- FFmpeg外部プロセスにフレームをパイプで渡す
- 既存の `ffmpeg-sidecar` または `std::process::Command` を使用

## アーキテクチャ

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────┐
│  RRD読み込み    │────▶│  ヘッドレス      │────▶│  FFmpeg     │
│  + Blueprint    │     │  レンダラー      │     │  (stdin)    │
└─────────────────┘     └──────────────────┘     └─────────────┘
                              │                        │
                              ▼                        ▼
                        各フレームRGBA            output.mp4
```

## 実装箇所

### CLI引数追加
**ファイル**: `crates/top/rerun/src/commands/entrypoint.rs`

```rust
#[clap(long)]
video_export: Option<PathBuf>,  // 出力MP4パス

#[clap(long, default_value = "30")]
fps: u32,  // フレームレート

#[clap(long, default_value = "1920x1080")]
resolution: String,  // 解像度
```

### 録画ロジック
**ファイル**: 新規 `crates/viewer/re_viewer/src/video_exporter.rs`

```rust
// 擬似コード
fn export_video(store: &EntityDb, output: &Path, fps: u32, resolution: (u32, u32)) {
    let timeline_range = store.time_range();
    let frame_duration = 1.0 / fps;
    let ffmpeg = spawn_ffmpeg(output, fps, resolution);

    for time in timeline_range.step_by(frame_duration) {
        // タイムラインを進める
        set_current_time(time);
        // フレームをレンダリング
        let frame_rgba = render_frame(resolution);
        // FFmpegにパイプ
        ffmpeg.stdin.write_all(&frame_rgba);
    }

    ffmpeg.wait();
}
```

### FFmpegコマンド

```bash
ffmpeg -f rawvideo -pix_fmt rgba -s 1920x1080 -r 30 \
       -i pipe:0 -c:v libx264 -pix_fmt yuv420p output.mp4
```

### 活用する既存コード
- `crates/viewer/re_viewer/src/screenshotter.rs`: フレームキャプチャ機構
- `crates/viewer/re_renderer/src/draw_phases/screenshot.rs`: GPU readback
- `crates/utils/re_video/src/decode/ffmpeg_cli/`: FFmpegサブプロセス管理

## エラーハンドリング

- **FFmpegが見つからない場合**: 明確なエラーメッセージ + インストール方法を表示
- **レンダリング失敗**: 途中のMP4を削除、エラー終了

## 将来の拡張（スコープ外）

- タイムライン範囲指定（`--start-time`, `--end-time`）
- 他のコーデック対応（H.265, VP9, AV1）
- 音声トラック対応
- プログレスバー表示
