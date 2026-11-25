//! What downloads keep beside a video: where its subtitles come from, and the copy on this PC as the player is told
//! of it. The player's own decisions are the shared core's (`zcore::play`).

/// A copy on this PC: its video, its sound when that's a file of its own, its subtitles' file.
#[derive(Clone, Debug)]
pub struct Playback {
    pub uri: String,
    pub sound: Option<String>,
    pub captions: Option<String>,
}


