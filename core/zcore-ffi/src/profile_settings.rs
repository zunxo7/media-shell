//! The profile's settings, its kept spots and the picture cropper, as Kotlin sees them (`zcore::profile_settings`,
//! `zcore::progress`, `zcore::profiles::Crop`).

use crate::screens::{screen, CoreRoot};
use std::collections::HashMap;
use std::sync::Arc;
use zcore::profile_settings::{
    Event as ProfileSettingsEvent, Setting as SettingValue, Settings as ProfileSettingsView,
};
use zcore::profiles::{Crop as CropFrame, Square as CropSquare};
use zcore::progress::Saved as CoreSaved;
use zcore::root::{ScreenId, Turn};
use zcore::screen::{Clock as CoreClock, Screen};

#[uniffi::remote(Enum)]
pub enum SettingValue {
    Flag { on: bool },
    Text { text: String },
    Number { number: f64 },
}

#[uniffi::remote(Record)]
pub struct ProfileSettingsView {
    pub profile: String,
    pub captions_on: bool,
    pub caption_size: String,
    pub caption_color: String,
    pub caption_background: f32,
    pub anime_audio: String,
    pub dramas_audio: String,
    pub autoplay_next: bool,
    pub quality: String,
    pub download_quality: String,
    pub changes: u32,
}

#[uniffi::remote(Enum)]
pub enum ProfileSettingsEvent {
    Started,
    Set { group: String, key: String, value: SettingValue },
}

screen!(
    /// The profile in use's settings (`zcore::profile_settings`): one for the app's whole life.
    ProfileSettingsScreen(zcore::profile_settings::ProfileSettings),
    ProfileSettingsEvent,
    ProfileSettingsView
);

/// The settings before the core has drawn them once.
#[uniffi::export]
pub fn profile_settings_default() -> ProfileSettingsView {
    ProfileSettingsView::default()
}

#[uniffi::remote(Record)]
pub struct CoreSaved {
    pub key: String,
    pub show: String,
    pub title: String,
    pub ep: Option<i64>,
    pub cover: Option<String>,
    pub at: f64,
    pub length: f64,
    pub updated: f64,
}

/// Where a profile's spots are kept on the phone (`zcore::progress::key`).
#[uniffi::export]
pub fn progress_key(profile: String) -> String {
    zcore::progress::key(&profile)
}

/// The spots in `/api/progress`'s answer, by key (`zcore::progress::spots`).
#[uniffi::export]
pub fn progress_spots(answer: String) -> HashMap<String, CoreSaved> {
    zcore::progress::spots(&answer)
}

/// The server's spots over those known here (`zcore::progress::merge`).
#[uniffi::export]
pub fn progress_merge(
    known: HashMap<String, CoreSaved>,
    server: HashMap<String, CoreSaved>,
) -> HashMap<String, CoreSaved> {
    zcore::progress::merge(&known, server)
}

#[uniffi::remote(Record)]
pub struct CropFrame {
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

#[uniffi::remote(Record)]
pub struct CropSquare {
    pub x: u32,
    pub y: u32,
    pub side: u32,
}

/// A picture just picked, framed on its whole middle square (`zcore::profiles::Crop`).
#[uniffi::export]
pub fn crop_start(width: f32, height: f32) -> CropFrame {
    CropFrame::new(width, height)
}

/// The zoom slider's place, 0 to 1.
#[uniffi::export]
pub fn crop_level(crop: CropFrame) -> f32 {
    crop.level()
}

/// Zoomed to the slider's place `level`.
#[uniffi::export]
pub fn crop_at_level(crop: CropFrame, level: f32) -> CropFrame {
    crop.at_level(level)
}

/// A drag and a pinch on a frame `frame` across: the spot under the fingers stays under them.
#[uniffi::export]
pub fn crop_moved(
    crop: CropFrame,
    frame: f32,
    centre_x: f32,
    centre_y: f32,
    pan_x: f32,
    pan_y: f32,
    by: f32,
) -> CropFrame {
    crop.moved(frame, (centre_x, centre_y), (pan_x, pan_y), by)
}

/// The square shown, in pixels of the picture read `scale` times as large.
#[uniffi::export]
pub fn crop_square(crop: CropFrame, scale: f32) -> CropSquare {
    crop.square(scale)
}

/// The square's side in the crop's own pixels.
#[uniffi::export]
pub fn crop_side(crop: CropFrame) -> f32 {
    crop.side()
}

/// The server page with a profile's spots (`zcore::progress::PATH`).
#[uniffi::export]
pub fn progress_path() -> String {
    zcore::progress::PATH.into()
}
