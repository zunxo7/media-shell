//! What downloads keep beside a video: where its subtitles come from, and the copy on this PC as the player is told
//! of it. The player's own decisions are the shared core's (`zcore::play`).
use crate::net::ApiError;

/// A copy on this PC: its video, its sound when that's a file of its own, its subtitles' file.
#[derive(Clone, Debug)]
pub struct Playback {
    pub uri: String,
    pub sound: Option<String>,
    pub captions: Option<String>,
}

pub fn error(message: impl Into<String>) -> ApiError {
    ApiError { message: message.into(), status: 0 }
}

/// What this PC can play, for the server to pick a file by. Everything, on purpose: mpv decodes whatever the
/// graphics card can't in software. (The phone's list differs on purpose: it names only its real hardware decoders,
/// since a weak box decoding HEVC or 10-bit in software stutters.)
pub const CODECS: [&str; 15] = [
    "h264", "hevc", "hevc10", "av1", "ac3", "eac3", "dts", "truehd", "pcm", "opus", "flac", "hdr", "hdr10", "local", "file",
];
