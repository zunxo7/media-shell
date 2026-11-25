/// Why a core call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The server couldn't be reached, or answered with an error. The text is the app's own description.
    #[error("{0}")]
    Network(String),
    /// The server answered, but not with what the core expected.
    #[error("unexpected answer from the server: {0}")]
    BadAnswer(String),
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::BadAnswer(error.to_string())
    }
}
