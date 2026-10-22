use std::fmt;

/// Everything that can go wrong.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Bad client setup, e.g. a protocol the provider does not offer or a missing API key.
    Config(String),
    /// Network or HTTP transport failure.
    Http(reqwest::Error),
    /// The server returned an error. `status` is 0 when the error arrived inside a stream.
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message extracted from the response body.
        message: String,
    },
    /// The response was not the JSON we expected, or a tool call carried invalid JSON.
    Json(serde_json::Error),
    /// MCP server failure.
    Mcp(String),
}

/// `Result` with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// HTTP status of an API error, if any.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            Error::Http(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(m) => write!(f, "config: {m}"),
            Error::Http(e) => write!(f, "http: {e}"),
            Error::Api { status: 0, message } => write!(f, "api error in stream: {message}"),
            Error::Api { status, message } => write!(f, "api error {status}: {message}"),
            Error::Json(e) => write!(f, "json: {e}"),
            Error::Mcp(m) => write!(f, "mcp: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Http(e) => Some(e),
            Error::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

/// Best-effort human message from an error body or error object.
pub(crate) fn message(body: &[u8]) -> String {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) => value_message(&v),
        Err(_) => String::from_utf8_lossy(&body[..body.len().min(2000)]).into_owned(),
    }
}

pub(crate) fn value_message(v: &serde_json::Value) -> String {
    let e = v.get("error").unwrap_or(v);
    e.get("message").and_then(|m| m.as_str()).or_else(|| e.as_str()).map_or_else(|| e.to_string(), str::to_owned)
}
