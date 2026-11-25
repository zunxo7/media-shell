//! What an older app left on the device, and how a newer one tidies it (once): the apps do the file work, this
//! decides what is moved and marked.

use std::collections::HashSet;

/// Written once the chapters saved on purpose are all marked kept.
pub const KEPT_FLAG: &str = ".kept-v1";
/// Written once the chapters saved under a personal Asura account have joined the rest.
pub const SHARED_FLAG: &str = ".shared-v1";
/// Where chapters were kept apart per account.
pub const ACCOUNTS_FOLDER: &str = "accounts";
/// Where Home was kept before the core's store kept it with the other pages.
pub const HOME_FILE: &str = "home.json";

/// A chapter found under an account's folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChapter {
    pub account: String,
    pub chapter: String,
    /// It holds the `complete` marker.
    pub complete: bool,
}

/// What the saved chapters' folder holds.
#[derive(Clone, Debug, Default)]
pub struct ChapterFacts {
    pub kept_flag: bool,
    pub shared_flag: bool,
    /// Every folder under the chapters' root holding the `complete` marker, relative to it ("a/b").
    pub complete_dirs: Vec<String>,
    /// Chapters under the accounts' folders, in the order found.
    pub account_chapters: Vec<AccountChapter>,
}

/// What to do about them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChapterPlan {
    /// Folders to put the `kept` marker in.
    pub mark_kept: Vec<String>,
    pub write_kept_flag: bool,
    /// Chapters to move up out of their account's folder, replacing what is there unless that is whole.
    pub joins: Vec<AccountChapter>,
    pub write_shared_flag: bool,
}

/// Chapters downloaded before there was a "kept" marker count as kept; chapters saved under a personal account (there
/// is none any more) join the shared ones, an incomplete copy never replacing a whole one.
pub fn chapter_plan(facts: &ChapterFacts) -> ChapterPlan {
    let mut plan = ChapterPlan::default();
    if !facts.kept_flag {
        plan.mark_kept = facts.complete_dirs.clone();
        plan.write_kept_flag = true;
    }
    if !facts.shared_flag {
        let mut whole: HashSet<&str> = facts.complete_dirs.iter().map(String::as_str).collect();
        for found in &facts.account_chapters {
            if whole.contains(found.chapter.as_str()) {
                continue;
            }
            plan.joins.push(found.clone());
            if found.complete {
                whole.insert(&found.chapter);
            }
        }
        plan.write_shared_flag = true;
    }
    plan
}

/// The file an older app kept this kept page in (moved into the store's folder if it's there and the page isn't).
pub fn old_page_file(key: &str) -> Option<&'static str> {
    (key == crate::home::KEY).then_some(HOME_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(account: &str, chapter: &str, complete: bool) -> AccountChapter {
        AccountChapter { account: account.into(), chapter: chapter.into(), complete }
    }

    #[test]
    fn old_chapters_are_marked_kept_and_account_ones_join_once() {
        let facts = ChapterFacts {
            kept_flag: false,
            shared_flag: false,
            complete_dirs: vec!["a".into(), "accounts/u/b".into()],
            account_chapters: vec![found("u", "a", true), found("u", "b", true), found("v", "b", false), found("w", "c", false)],
        };
        let plan = chapter_plan(&facts);
        assert_eq!(plan.mark_kept, ["a", "accounts/u/b"]);
        // ("a" is whole already; "b" joins whole, so the account's other copy stays out; "c" joins)
        assert_eq!(plan.joins, [found("u", "b", true), found("w", "c", false)]);
        assert!(plan.write_kept_flag && plan.write_shared_flag);
        let done = chapter_plan(&ChapterFacts { kept_flag: true, shared_flag: true, ..facts });
        assert_eq!(done, ChapterPlan::default());
    }

    #[test]
    fn only_home_was_kept_beside_the_pages() {
        assert_eq!(old_page_file("home"), Some("home.json"));
        assert_eq!(old_page_file("me-progress-p1"), None);
    }
}
