//! The `ffmpeg` encoder of a virtual display (SPEC-002 ADR-15).
//!
//! `ffmpeg` grabs the display with x11grab (shared memory, without the pointer: the viewer
//! draws its own cursor) and encodes H.264 with libx264 for latency: `ultrafast`,
//! `zerolatency` (no lookahead, sliced threads), no B-frames, one IDR every keyframe interval.
//! The colour conversion goes through packed BGR first, which takes swscale's fast path (about
//! half the CPU of a direct `bgr0` → `yuv420p` conversion, measured on the VM). Output is FLV on
//! stdout, flushed per packet ([`crate::flv`]).
//!
//! Ownership: one supervised process (own process group, bounded stderr tail) and one reader
//! task per run; [`EncoderStop`] terminates the group, escalates to SIGKILL and reaps it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::flv::{FlvItem, FlvParser, avc_codec_string};
use crate::process::{StopTimeouts, Supervised, passed_env};
use crate::video::{
    ENCODER_EVENTS, EncoderEvent, EncoderStart, EncoderStop, RunningEncoder, VideoConfig,
    VideoFrame,
};

/// Bytes read from the encoder per call.
const READ_CHUNK: usize = 256 * 1024;
/// How long the encoder may take to stop after SIGTERM before SIGKILL.
const STOP: StopTimeouts = StopTimeouts {
    graceful: Duration::ZERO,
    term: Duration::from_secs(2),
    kill: Duration::from_secs(5),
};

/// Encoder parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FfmpegStart {
    /// The `ffmpeg` executable.
    pub(crate) program: PathBuf,
    /// The display, e.g. `:101`.
    pub(crate) display: String,
    /// The display's cookie file.
    pub(crate) xauthority: PathBuf,
    /// Frames per second.
    pub(crate) fps: u32,
    /// Frames between keyframes.
    pub(crate) keyframe_frames: u32,
}

impl FfmpegStart {
    /// The argument vector for a capture of `width`×`height` device pixels at the top left.
    pub(crate) fn args(&self, (width, height): (u32, u32)) -> Vec<OsString> {
        let fps = self.fps.to_string();
        let gop = self.keyframe_frames.max(1).to_string();
        let size = format!("{width}x{height}");
        let input = format!("{}.0+0,0", self.display);
        [
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-nostats",
            "-fflags",
            "nobuffer",
            "-flags",
            "low_delay",
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-f",
            "x11grab",
            "-draw_mouse",
            "0",
            "-framerate",
            &fps,
            "-video_size",
            &size,
            "-i",
            &input,
            "-vf",
            "format=bgr24,format=yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-bf",
            "0",
            "-g",
            &gop,
            "-threads",
            "2",
            "-colorspace",
            "smpte170m",
            "-color_range",
            "tv",
            "-flush_packets",
            "1",
            "-f",
            "flv",
            "-flvflags",
            "no_duration_filesize",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }
}

struct FfmpegStop {
    process: Supervised,
    reader: JoinHandle<()>,
}

impl EncoderStop for FfmpegStop {
    fn stop(
        self: Box<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> {
        Box::pin(async move {
            let outcome = self.process.terminate(STOP).await;
            self.reader.abort();
            match outcome {
                Ok(status) => {
                    debug!(%status, "display encoder reaped");
                    None
                }
                Err(_) => Some(format!(
                    "ffmpeg (pid {}) survived SIGKILL",
                    self.process.pid()
                )),
            }
        })
    }
}

impl EncoderStart for FfmpegStart {
    async fn start(&self, size: (u32, u32)) -> Result<RunningEncoder, String> {
        let mut env = passed_env();
        env.retain(|(name, _)| name != "DISPLAY" && name != "XAUTHORITY");
        env.push((
            "XAUTHORITY".into(),
            self.xauthority.clone().into_os_string(),
        ));
        let (process, stdout) = Supervised::spawn_piped(&self.program, &self.args(size), &env)
            .map_err(|error| format!("cannot start {}: {error}", self.program.display()))?;
        let (tx, events) = mpsc::channel(ENCODER_EVENTS);
        let reader = tokio::spawn(read_stream(stdout, tx, size));
        Ok(RunningEncoder {
            events,
            stop: Box::new(FfmpegStop { process, reader }),
        })
    }
}

/// Parses the encoder's output into events until it ends or breaks the format.
async fn read_stream(
    mut stdout: tokio::process::ChildStdout,
    tx: mpsc::Sender<EncoderEvent>,
    (width, height): (u32, u32),
) {
    let mut parser = FlvParser::new();
    let mut buffer = vec![0_u8; READ_CHUNK];
    loop {
        let read = match stdout.read(&mut buffer).await {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) => {
                debug!(%error, "reading the display encoder failed");
                return;
            }
        };
        let items = match parser.push(&buffer[..read]) {
            Ok(items) => items,
            Err(error) => {
                warn!(%error, "display encoder output is malformed; restarting it");
                return;
            }
        };
        for item in items {
            let event = match item {
                FlvItem::AvcConfig(record) => {
                    let Some(codec) = avc_codec_string(&record) else {
                        warn!("display encoder sent an invalid configuration");
                        return;
                    };
                    EncoderEvent::Config(Arc::new(VideoConfig {
                        codec,
                        description: record.into(),
                        width,
                        height,
                    }))
                }
                FlvItem::Video {
                    keyframe,
                    timestamp_ms,
                    data,
                } => EncoderEvent::Frame(VideoFrame {
                    keyframe,
                    timestamp_us: u64::from(timestamp_ms) * 1000,
                    data: data.into(),
                }),
            };
            if tx.send(event).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_capture_the_top_left_region_without_the_pointer_at_low_latency() {
        let start = FfmpegStart {
            program: "ffmpeg".into(),
            display: ":101".into(),
            xauthority: "/data/displays/s1/Xauthority".into(),
            fps: 60,
            keyframe_frames: 120,
        };
        let args: Vec<String> = start
            .args((1280, 800))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        for expected in [
            "-f x11grab -draw_mouse 0 -framerate 60 -video_size 1280x800 -i :101.0+0,0",
            "-c:v libx264 -preset ultrafast -tune zerolatency -bf 0 -g 120",
            "-flush_packets 1 -f flv",
        ] {
            assert!(joined.contains(expected), "{expected} in {joined}");
        }
        assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
        assert!(
            !joined.contains("Xauthority"),
            "the cookie path is not an argument"
        );
    }
}
