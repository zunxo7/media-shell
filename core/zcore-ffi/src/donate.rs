//! Donations (`zcore::donate`) as Kotlin sees them. `Goal` and `Detail` get a prefix here: too plain a name beside
//! the app's own.

use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::donate::{Detail as DonateDetail, Event as DonateEvent, Goal as DonateGoal, View as DonateView};
use zcore::root::{ScreenId, Turn};
use zcore::screen::Clock as CoreClock;
use zcore::screen::Screen;

#[uniffi::remote(Enum)]
pub enum DonateEvent {
    Opened,
    Refresh,
    Copied { index: u32 },
    Bank,
}

#[uniffi::remote(Record)]
pub struct DonateView {
    pub on: bool,
    pub button: String,
    pub line: String,
    pub goal: Option<DonateGoal>,
    pub bank: String,
    pub bank_open: bool,
    pub details: Vec<DonateDetail>,
    pub tv: bool,
}

#[uniffi::remote(Record)]
pub struct DonateGoal {
    pub share: f32,
    pub raised: String,
    pub of: String,
}

#[uniffi::remote(Record)]
pub struct DonateDetail {
    pub label: String,
    pub value: String,
    pub copy: String,
    pub copied: bool,
    pub copyable: bool,
    pub name: bool,
}

screen!(
    /// Donations: the button, and its panel's goal and bank details.
    DonateScreen(zcore::donate::Donate),
    DonateEvent,
    DonateView
);

/// The sheet that asks for a donation (`zcore::nav::DONATE`).
#[uniffi::export]
pub fn donate_sheet() -> String {
    zcore::nav::DONATE.into()
}
