//! What an older app left on the device, and how it is tidied (`zcore::legacy`), as Kotlin sees it.

use zcore::legacy::{self, AccountChapter, ChapterFacts, ChapterPlan};

#[uniffi::remote(Record)]
pub struct AccountChapter {
    pub account: String,
    pub chapter: String,
    pub complete: bool,
}

#[uniffi::remote(Record)]
pub struct ChapterFacts {
    pub kept_flag: bool,
    pub shared_flag: bool,
    pub complete_dirs: Vec<String>,
    pub account_chapters: Vec<AccountChapter>,
}

#[uniffi::remote(Record)]
pub struct ChapterPlan {
    pub mark_kept: Vec<String>,
    pub write_kept_flag: bool,
    pub joins: Vec<AccountChapter>,
    pub write_shared_flag: bool,
}

/// The names the chapters' folder was tidied under (`zcore::legacy`).
#[derive(Debug, uniffi::Record)]
pub struct LegacyNames {
    pub kept_flag: String,
    pub shared_flag: String,
    pub accounts: String,
}

#[uniffi::export]
pub fn legacy_names() -> LegacyNames {
    LegacyNames { kept_flag: legacy::KEPT_FLAG.into(), shared_flag: legacy::SHARED_FLAG.into(), accounts: legacy::ACCOUNTS_FOLDER.into() }
}

/// What to do about the saved chapters an older app left (`zcore::legacy::chapter_plan`).
#[uniffi::export]
pub fn legacy_chapter_plan(facts: ChapterFacts) -> ChapterPlan {
    legacy::chapter_plan(&facts)
}

/// The file an older app kept a page in (`zcore::legacy::old_page_file`).
#[uniffi::export]
pub fn legacy_page_file(key: String) -> Option<String> {
    legacy::old_page_file(&key).map(Into::into)
}
