//! The installer's words and rules (Windows' bootstrapper: installing, updating, uninstalling): what each step says
//! and how far its bar is. The files and the window are the shell's (`platform::install`).

/// Where the installer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Installing,
    Done,
}

/// What the bootstrapper's one panel says: its title, the line under it and how far the bar is (none: no bar).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Say {
    pub title: String,
    pub detail: String,
    pub share: Option<f32>,
}

pub const TRY_AGAIN: &str = "Try again";
pub const CANCEL: &str = "Cancel";
pub const UNINSTALL: &str = "Uninstall";
/// An update's last part, the same on every OS: Windows' setup shows it, the Mac app itself while it unpacks.
pub const FINISHING: &str = "Finishing up";

/// How much of an update's one bar the download fills (the app's part); the installer fills the rest.
pub const DOWNLOADED: f32 = 0.85;
/// The least time the install's bar takes to fill: a quick install still shows itself.
pub const MIN_SHOW_MS: f32 = 3000.;
/// How long "Starting" stays before the new app opens.
pub const OPEN_AFTER_MS: u64 = 700;

fn say(title: &str, detail: impl Into<String>, share: Option<f32>) -> Say {
    Say { title: title.into(), detail: detail.into(), share }
}

/// An update's bar while the app downloads it: the first part of the one bar.
pub fn download_bar(share: f32) -> f32 {
    share * DOWNLOADED
}

/// An update's bar once the installer carries on from where the download left it.
pub fn finish_bar(share: f32) -> f32 {
    DOWNLOADED + (1. - DOWNLOADED) * share
}

/// What the app says while it makes an update ready itself (the Mac's), before it closes.
pub fn finishing(share: f32) -> Say {
    say(crate::update::UPDATING, FINISHING, Some(finish_bar(share)))
}

/// Installing, or the last part of an update (`updating`); `total` bytes in all.
pub fn installing(phase: Phase, updating: bool, share: f32, total: f64) -> Say {
    match phase {
        Phase::Done if updating => say(crate::update::UPDATING, FINISHING, Some(1.)),
        Phase::Done => say("Starting", "", Some(1.)),
        _ if updating => finishing(share),
        _ => say("Installing", format!("{:.1} MB of {:.1} MB", total * f64::from(share) / 1e6, total / 1e6), Some(share)),
    }
}

/// Uninstalling: asked first, then the files taken away so far (of how many), then done.
pub fn uninstalling(phase: Phase, files: (usize, usize), share: f32) -> Say {
    match phase {
        Phase::Ready => say("", "", None),
        Phase::Installing => {
            let (done, all) = files;
            say("Uninstalling", if all > 0 { format!("{done} of {all} files") } else { "Waiting for zunflix to close".into() }, Some(share))
        }
        Phase::Done => say("Removed", "", Some(1.)),
    }
}

/// The share of the files taken away.
pub fn files_share(done: usize, all: usize) -> f32 {
    if all > 0 { done as f32 / all as f32 } else { 1. }
}

/// The box that deletes the downloads too: "Also delete downloads (2.8 GB)".
pub fn delete_downloads(bytes: u64) -> String {
    let size = if bytes >= 1_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { format!("{} MB", (bytes as f64 / 1e6).round().max(1.)) };
    format!("Also delete downloads ({size})")
}

/// Said when the install can't make its folder; `cause` is the system's own reason.
pub fn folder_failed(cause: &str) -> String {
    format!("Couldn't make the folder: {cause}")
}

/// Said when the app can't be put in place (a copy of it still open, most often); `cause` is the system's own reason.
pub fn place_failed(cause: &str) -> String {
    format!("Couldn't put zunflix in place ({cause}). Close zunflix and try again.")
}

/// A Mac's prompt when the app runs from its disk image: its title, its line and its two buttons.
pub const MOVE_TITLE: &str = "Move zunflix to Applications?";
pub const MOVE_LINE: &str = "It's running from its disk image. In Applications it stays put and keeps itself up to date.";
pub const MOVE_BUTTON: &str = "Move to Applications";
pub const MOVE_LATER: &str = "Not Now";

/// The one line under the bootstrapper's name: its title and detail, or a blank (the line keeps its room).
pub fn line(title: &str, detail: &str) -> String {
    let said = if detail.is_empty() { title.to_string() } else { format!("{title}  ·  {detail}") };
    if said.trim().is_empty() { " ".into() } else { said }
}

/// How far the install's bar shows: the real share, never faster than across in [`MIN_SHOW_MS`].
pub fn shown_share(real: f32, elapsed_ms: f32) -> f32 {
    real.min(elapsed_ms / MIN_SHOW_MS)
}

/// How long Removed stays: never under 4 s since the window opened (so the downloads box can be ticked), at least 1.3 s.
pub fn removed_wait_ms(opened_ms: u64) -> u64 {
    4000u64.saturating_sub(opened_ms).max(1300)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_failures_say_why() {
        assert_eq!(folder_failed("denied"), "Couldn't make the folder: denied");
        assert_eq!(place_failed("in use"), "Couldn't put zunflix in place (in use). Close zunflix and try again.");
    }

    #[test]
    fn an_update_is_one_bar_in_two_parts() {
        assert_eq!(download_bar(1.), DOWNLOADED);
        assert_eq!(finish_bar(0.), DOWNLOADED);
        assert_eq!(finish_bar(1.), 1.);
        let s = installing(Phase::Installing, true, 0.5, 0.);
        assert_eq!((s.title.as_str(), s.detail.as_str()), ("Updating", "Finishing up"));
    }

    #[test]
    fn installing_says_how_many_megabytes() {
        let s = installing(Phase::Installing, false, 0.5, 14.2e6);
        assert_eq!((s.title.as_str(), s.detail.as_str(), s.share), ("Installing", "7.1 MB of 14.2 MB", Some(0.5)));
        assert_eq!(installing(Phase::Done, false, 1., 0.).title, "Starting");
    }

    #[test]
    fn uninstalling_counts_files_and_waits() {
        assert_eq!(uninstalling(Phase::Installing, (3, 10), 0.3).detail, "3 of 10 files");
        assert_eq!(uninstalling(Phase::Installing, (0, 0), 1.).detail, "Waiting for zunflix to close");
        assert_eq!(uninstalling(Phase::Ready, (0, 0), 0.).share, None);
        assert_eq!(files_share(0, 0), 1.);
    }

    #[test]
    fn the_line_joins_title_and_detail() {
        assert_eq!(line("Installing", "1 of 2"), "Installing  ·  1 of 2");
        assert_eq!(line("Starting", ""), "Starting");
        assert_eq!(line("", ""), " ");
    }

    #[test]
    fn the_downloads_box_names_their_size() {
        assert_eq!(delete_downloads(2_800_000_000), "Also delete downloads (2.8 GB)");
        assert_eq!(delete_downloads(640_000_000), "Also delete downloads (640 MB)");
        assert_eq!(delete_downloads(0), "Also delete downloads (1 MB)");
    }

    #[test]
    fn the_bar_never_outruns_its_minimum_time() {
        assert_eq!(shown_share(1., 1500.), 0.5);
        assert_eq!(shown_share(0.2, 3000.), 0.2);
        assert_eq!((removed_wait_ms(0), removed_wait_ms(3900), removed_wait_ms(9000)), (4000, 1300, 1300));
    }
}
