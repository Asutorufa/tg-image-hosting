use frankenstein::reqwest;
use std::fmt::Display;

#[derive(Debug)]
pub struct Error(pub String);

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Error {
    fn from(err: String) -> Self {
        Error(err)
    }
}

impl From<worker::Error> for Error {
    fn from(err: worker::Error) -> Self {
        Error(err.to_string())
    }
}

impl From<frankenstein::Error> for Error {
    fn from(err: frankenstein::Error) -> Self {
        Error(err.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error(err.to_string())
    }
}
