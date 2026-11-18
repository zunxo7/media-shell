//! Play, here or on the account's TV: while Play on TV is on, the core (`zcore::your_tv`) starts it there.
use crate::data::{Episode, Title};
use crate::{player, App};
use std::sync::Arc;

/// Play: on the TV while Play on TV is on, else here. Downloads always play here (they call `player::open`).
pub fn play(app: &mut App, title: Arc<Title>, ep: Option<Episode>) {
    if app.pages.your_tv.view().plays_on_tv {
        app.pages.your_tv.send(zcore::your_tv::Event::Play { title: title.id.clone(), episode: ep.map(|e| e.id) });
    } else {
        player::open(app, title, ep);
    }
}
