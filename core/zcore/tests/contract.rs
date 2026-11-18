//! The server's real answers (tests/fixtures/<device>/, saved by tools/capture_fixtures.py) read by the core's own
//! parsers, for each kind of device the server answers differently. A change on the server the apps can't read fails
//! here, not in an app.

use serde_json::Value;
use std::path::PathBuf;

const DEVICES: [&str; 3] = ["phone", "tv", "desktop"];

fn folder(device: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(device)
}

fn text(device: &str, name: &str) -> String {
    let path = folder(device).join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn json(device: &str, name: &str) -> Value {
    serde_json::from_str(&text(device, name)).unwrap_or_else(|e| panic!("{device}/{name}: {e}"))
}

#[test]
fn home_has_a_stage_and_rows_of_titles() {
    for device in DEVICES {
        let home = zcore::home::parse(&json(device, "home"));
        assert!(!home.hero.is_empty() && home.hero.len() <= zcore::home::HERO_SLIDES, "{device}: hero");
        assert!(!home.rows.is_empty(), "{device}: rows");
        for row in &home.rows {
            assert!(!row.title.is_empty(), "{device}: a row without a name");
            assert!(row.items.iter().all(|i| !i.id.is_empty() && !i.title.is_empty()), "{device}: {}", row.title);
        }
    }
}

#[test]
fn every_kind_of_title_page_reads() {
    for device in DEVICES {
        let mut kinds = 0;
        for entry in std::fs::read_dir(folder(device)).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().trim_end_matches(".json").to_string();
            let Some(kind) = name.strip_prefix("title-") else { continue };
            kinds += 1;
            let page = json(device, &name);
            let id = page.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let title = zcore::model::title(&id, &page);
            assert!(!title.title.is_empty(), "{device}/{kind}: no name");
            if kind.ends_with("-show") {
                assert!(!title.episodes.is_empty(), "{device}/{kind}: a show without episodes");
            }
        }
        assert!(kinds >= 4, "{device}: only {kinds} kinds of title");
    }
}

#[test]
fn lists_read() {
    for device in DEVICES {
        for (name, key) in [("anime", "items"), ("cinema-movies", "items"), ("cinema-shows", "items")] {
            let items = zcore::model::items(&json(device, name), key);
            assert!(!items.is_empty(), "{device}/{name}: empty");
            assert!(items.iter().all(|i| !i.id.is_empty()), "{device}/{name}: an item without an id");
        }
    }
}

#[test]
fn saved_spots_and_up_next_read() {
    for device in DEVICES {
        let saved = zcore::progress::saved(&text(device, "progress"));
        assert!(saved.is_ok(), "{device}/progress: {saved:?}");
        // (whatever the test account last watched: the answer reads, whether it has any or not)
        let _ = zcore::home::up_next(&json(device, "up-next"));
    }
}
