//! Words an app's controls carry that no screen's view holds: a control's accessible name, a button, a prompt.

/// The words of the controls every page shares.
#[derive(Clone, Debug, PartialEq)]
pub struct Words {
    /// The back button's name.
    pub back: String,
    /// A failed download's (or any retry) button.
    pub try_again: String,
    /// The profile picture's button in the bar.
    pub profiles_and_settings: String,
    /// What the voice search asks.
    pub say_a_title: String,
}

pub fn words() -> Words {
    Words {
        back: "Back".into(),
        try_again: "Try again".into(),
        profiles_and_settings: "Profiles and settings".into(),
        say_a_title: "Say a title".into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shared_words_are_said() {
        let w = super::words();
        assert_eq!((w.back.as_str(), w.profiles_and_settings.as_str()), ("Back", "Profiles and settings"));
    }
}
