//! zunflix's colours: the one grey look (no themes to pick). "flix" in the name is the wordmark grey; what's on,
//! badges, how far in and the player's bar are a soft white with black words on it, as the phone has them.

pub struct Theme {
    pub accent: u32,
    /// What's written on the accent.
    pub ink: u32,
}

const GREY: Theme = Theme { accent: crate::tokens::C_ACCENT, ink: crate::tokens::C_BLACK };

pub fn accent() -> u32 {
    GREY.accent
}

pub fn ink() -> u32 {
    GREY.ink
}

/// Deleting, removing, signing out, errors: a clean deep red (the shared token's soft salmon read as pink).
pub const DANGER: u32 = crate::tokens::C_DANGER;

/// "flix" in the name.
pub fn flix() -> u32 {
    crate::tokens::C_WORDMARK_GREY
}

