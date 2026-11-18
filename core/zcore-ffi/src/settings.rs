//! The updater (`zcore::update`) and the Settings page (`zcore::settings`) as Kotlin sees them. Names that clash with
//! the app's own (its `Switch`, `Choice`, `Danger` and `Downloads`) get a prefix here.

use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::root::{ScreenId, Turn};
use zcore::screen::Clock as CoreClock;
use zcore::screen::Screen;
use zcore::settings::{
    Choice as SettingsChoice, Danger as DangerButton, DeviceKind as SignedDeviceKind, DeviceRow as SignedDevice,
    Devices as SettingsDevices, Downloads as SettingsDownloads, Event as SettingsEvent, Head as TileHead,
    Languages as SettingsLanguages, Part as SettingsPart, Pick as SettingsPick, Playback as SettingsPlayback,
    Subtitles as SettingsSubtitles, Switch as SettingSwitch, View as SettingsView,
};
use zcore::update::{
    Bootstrap as UpdateBootstrap, Build as UpdateBuild, Card as UpdateCard, Event as UpdaterEvent,
    VersionCard as AppVersionCard, VersionOffer as AppVersionOffer, View as UpdaterView,
};

#[uniffi::remote(Enum)]
pub enum UpdaterEvent {
    Opened,
    Playing { playing: bool },
    Update,
    Later,
    Progress { done: u64, total: u64 },
    Fetched,
    Failed,
}

#[uniffi::remote(Record)]
pub struct UpdateBuild {
    pub code: i64,
    pub url: String,
}

#[uniffi::remote(Record)]
pub struct UpdateCard {
    pub title: String,
    pub version: String,
    pub line: String,
    pub button: String,
    pub later: String,
    pub updating: bool,
    pub share: f32,
    pub percent: String,
}

#[uniffi::remote(Record)]
pub struct UpdateBootstrap {
    pub title: String,
    pub line: String,
    pub share: f32,
}

#[uniffi::remote(Record)]
pub struct AppVersionCard {
    pub name: String,
    pub line: String,
    pub up_to_date: String,
    pub offer: Option<AppVersionOffer>,
}

#[uniffi::remote(Record)]
pub struct AppVersionOffer {
    pub button: String,
    pub version: String,
}

#[uniffi::remote(Record)]
pub struct UpdaterView {
    pub card: Option<UpdateCard>,
    pub rail: Option<String>,
    pub version: AppVersionCard,
    pub bootstrap: Option<UpdateBootstrap>,
    pub note: Option<String>,
    pub fetch: Option<UpdateBuild>,
    pub install: Option<UpdateBuild>,
    pub installs: u32,
    pub booting: bool,
    pub opening: bool,
    pub up_to_date: bool,
}

screen!(
    /// The app's updater (`zcore::update::Updater`): one for the app's run.
    UpdaterScreen(zcore::update::Updater),
    UpdaterEvent,
    UpdaterView
);

#[uniffi::remote(Enum)]
pub enum SettingsPart {
    Profiles,
    Subtitles,
    Playback,
    Downloads,
    Languages,
    Devices,
    Version,
}

#[uniffi::remote(Record)]
pub struct SettingsPick {
    pub value: String,
    pub label: String,
}

#[uniffi::remote(Record)]
pub struct SettingsChoice {
    pub group: String,
    pub key: String,
    pub name: String,
    pub options: Vec<SettingsPick>,
}

#[uniffi::remote(Record)]
pub struct SettingSwitch {
    pub group: String,
    pub key: String,
    pub name: String,
    pub line: String,
}

#[uniffi::remote(Record)]
pub struct TileHead {
    pub title: String,
    pub line: String,
}

#[uniffi::remote(Record)]
pub struct SettingsSubtitles {
    pub head: TileHead,
    pub preview: String,
    pub on: SettingSwitch,
    pub size: SettingsChoice,
    pub colour: SettingsChoice,
    pub background: SettingsChoice,
}

#[uniffi::remote(Record)]
pub struct SettingsPlayback {
    pub head: TileHead,
    pub autoplay: SettingSwitch,
    pub quality: SettingsChoice,
}

#[uniffi::remote(Record)]
pub struct SettingsLanguages {
    pub head: TileHead,
    pub anime: SettingsChoice,
    pub dramas: SettingsChoice,
}

#[uniffi::remote(Record)]
pub struct SettingsDownloads {
    pub head: TileHead,
    pub quality: SettingsChoice,
    pub mobile_data: Option<SettingSwitch>,
}

#[uniffi::remote(Enum)]
pub enum SignedDeviceKind {
    Phone,
    Tv,
    Computer,
}

#[uniffi::remote(Record)]
pub struct DangerButton {
    pub label: String,
    pub confirm: String,
    pub asking: bool,
}

#[uniffi::remote(Record)]
pub struct SignedDevice {
    pub id: String,
    pub name: String,
    pub kind: SignedDeviceKind,
    pub line: String,
    pub remove: Option<DangerButton>,
}

#[uniffi::remote(Record)]
pub struct SettingsDevices {
    pub head: TileHead,
    pub loading: bool,
    pub count: Option<String>,
    pub rows: Vec<SignedDevice>,
    pub others: Option<DangerButton>,
    pub note: Option<String>,
}

#[uniffi::remote(Record)]
pub struct SettingsView {
    pub title: String,
    pub single: Vec<SettingsPart>,
    pub left: Vec<SettingsPart>,
    pub right: Vec<SettingsPart>,
    pub saved: Option<String>,
    pub subtitles: SettingsSubtitles,
    pub playback: SettingsPlayback,
    pub languages: SettingsLanguages,
    pub downloads: Option<SettingsDownloads>,
    pub devices: Option<SettingsDevices>,
}

#[uniffi::remote(Enum)]
pub enum SettingsEvent {
    Opened { plus: bool },
    Changed,
    Remove { id: String },
    RemoveOthers,
}

screen!(
    /// The Settings page (`zcore::settings::Settings`).
    SettingsScreen(zcore::settings::Settings),
    SettingsEvent,
    SettingsView
);
