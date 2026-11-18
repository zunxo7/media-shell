//! Search as you type: names compared without case or accents, and titles ranked by how well they match.

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// Lower case, with accents taken off: Unicode's NFKD splits a letter from its marks, which go.
pub fn fold(s: &str) -> String {
    s.nfkd().filter(|c| !is_combining_mark(*c)).collect::<String>().to_lowercase()
}

/// How well a title matches the folded query `q` (and its `words`), its name and genres folded too: the name
/// starting with it (0), containing it (1), every word in the name (2), every word in the name or genres (3); 4
/// doesn't match.
pub fn rank(title: &str, genres: &str, q: &str, words: &[&str]) -> u8 {
    if title.starts_with(q) {
        0
    } else if title.contains(q) {
        1
    } else if words.iter().all(|w| title.contains(w)) {
        2
    } else if words.iter().all(|w| title.contains(w) || genres.contains(w)) {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_by_how_it_matches() {
        let words = ["blue"];
        assert_eq!(rank("blue box", "romance", "blue", &words), 0);
        assert_eq!(rank("the blue lagoon", "drama", "blue", &words), 1);
        assert_eq!(rank("lagoon", "blue drama", "blue", &words), 3);
        assert_eq!(rank("nothing", "comedy", "blue", &words), 4);
        assert_eq!(rank("box of blue", "", "box blue", &["box", "blue"]), 2);
    }

    #[test]
    fn folds_accents_and_case() {
        assert_eq!(fold("Amélie ÉCOLE"), "amelie ecole");
    }
}
