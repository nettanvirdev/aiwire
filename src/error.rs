use std::{fmt, time::Duration};

use reqwest::header::HeaderMap;
use serde_json::Value;

/// Everything that can go wrong.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Bad client setup, e.g. a protocol the provider does not offer or a missing API key.
    Config(String),
    /// Network or HTTP transport failure (connect, TLS, reset, whole-request timeout).
    Http(reqwest::Error),
    /// The server answered with an error, or with something that was not the expected JSON.
    /// Holds everything known about the failure; print it with `{:#?}` when debugging.
    Api(Box<ApiError>),
    /// No bytes from the server for this long: a stalled stream, or a server that never answered.
    Timeout(Duration),
    /// Invalid JSON you supplied, e.g. tool-call arguments that do not parse.
    Json(serde_json::Error),
    /// MCP server failure.
    Mcp(String),
}

/// A failed API call, with everything needed to debug it or report it to the provider.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ApiError {
    /// HTTP status. 0 when the error arrived inside an already-open stream.
    pub status: u16,
    /// Provider error type or code, e.g. `rate_limit_error`, `invalid_api_key`, `overloaded_error`.
    /// `invalid_response` when the body was not the JSON we expected.
    pub kind: Option<String>,
    /// Human-readable message from the provider.
    pub message: String,
    /// Request id (from headers or body). Quote it when contacting the provider.
    pub request_id: Option<String>,
    /// Provider name, or `custom` for a bare base URL.
    pub provider: &'static str,
    /// Endpoint that was called.
    pub url: String,
    /// How long the server asked us to wait (`retry-after`).
    pub retry_after: Option<Duration>,
    /// Response headers useful for debugging: rate limits, request ids, `location`, `retry-after`.
    pub headers: Vec<(String, String)>,
    /// Raw response body (first 8 KiB), for whatever the fields above miss.
    pub body: String,
    /// Attempts made, retries included.
    pub attempts: u32,
}

/// `Result` with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// HTTP status of an API error, if any.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api(e) => Some(e.status),
            Error::Http(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }

    /// The API error details, if this is one.
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Error::Api(e) => Some(e),
            _ => None,
        }
    }

    /// Request id of a failed API call, if the provider sent one.
    pub fn request_id(&self) -> Option<&str> {
        self.api()?.request_id.as_deref()
    }

    /// Whether trying again later may succeed: network failures, timeouts, rate limits,
    /// overload and server errors (in streams too). The client already retried these
    /// `max_retries` times before returning (except failures in the middle of a stream).
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Http(e) => e.is_connect() || e.is_timeout() || e.is_request() || e.is_body(),
            Error::Timeout(_) => true,
            Error::Api(e) => {
                matches!(e.status, 408 | 409 | 429 | 500..=599)
                    || matches!(
                        e.kind.as_deref(),
                        Some("overloaded_error" | "rate_limit_error" | "api_error" | "server_error" | "timeout")
                    ) && !matches!(e.status, 400..=428 | 430..=499)
            }
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(m) => write!(f, "config: {m}"),
            Error::Http(e) => {
                write!(f, "http: {e}")?;
                // reqwest's top-level message is generic; the cause says what actually failed.
                let mut src = std::error::Error::source(e);
                while let Some(s) = src {
                    write!(f, ": {s}")?;
                    src = s.source();
                }
                Ok(())
            }
            Error::Api(e) => e.fmt(f),
            Error::Timeout(d) => write!(
                f,
                "timed out: no data from the server for {}s (raise ClientBuilder::stream_idle_timeout for models that think silently longer)",
                d.as_secs_f32()
            ),
            Error::Json(e) => write!(f, "json: {e}"),
            Error::Mcp(m) => write!(f, "mcp: {m}"),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let provider = if self.provider.is_empty() { "api" } else { self.provider };
        match self.status {
            0 => write!(f, "{provider} error in stream")?,
            s => write!(f, "{provider} error {s}")?,
        }
        if let Some(k) = &self.kind {
            write!(f, " {k}")?;
        }
        write!(f, ": {}", self.message)?;
        let mut sep = " (";
        let mut item = |f: &mut fmt::Formatter<'_>, args: fmt::Arguments<'_>| {
            let r = write!(f, "{sep}{args}");
            sep = ", ";
            r
        };
        if !self.url.is_empty() {
            item(f, format_args!("{}", self.url))?;
        }
        if let Some(id) = &self.request_id {
            item(f, format_args!("request id {id}"))?;
        }
        if self.attempts > 1 {
            item(f, format_args!("{} attempts", self.attempts))?;
        }
        if let Some(d) = self.retry_after {
            item(f, format_args!("retry after {}s", d.as_secs_f32()))?;
        }
        if sep == ", " {
            f.write_str(")")?;
        }
        match self.status {
            301..=303 => {
                let to = self.header("location").unwrap_or("?");
                write!(f, "; the server redirects to {to}: use that as the base URL")?;
            }
            404 if self.kind.is_none() => {
                f.write_str(
                    "; check the base URL (aiwire appends /chat/completions, /v1/messages, /responses or /embeddings)",
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// A response header kept in [`headers`](Self::headers), by lowercase name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Error from an HTTP body; fills status, message, kind, request id and body.
    pub(crate) fn from_body(status: u16, body: &[u8]) -> Self {
        let mut e = match serde_json::from_slice::<Value>(body) {
            Ok(v) => Self::from_value(&v),
            Err(_) => {
                let text = String::from_utf8_lossy(&body[..body.len().min(500)]);
                Self { message: text.trim().to_owned(), ..Self::default() }
            }
        };
        if e.message.is_empty() {
            e.message = format!("HTTP {status}");
        }
        e.status = status;
        e.body = String::from_utf8_lossy(&body[..body.len().min(8192)]).into_owned();
        e
    }

    /// Error from a JSON error object (`{"error": {...}}` or the error itself).
    pub(crate) fn from_value(v: &Value) -> Self {
        let e = v.get("error").filter(|e| !e.is_null()).unwrap_or(v);
        let str_of = |k: &str| match e.get(k) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let message = e
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| e.as_str())
            .or_else(|| v.pointer("/base_resp/status_msg").and_then(Value::as_str))
            .map_or_else(|| e.to_string(), str::to_owned);
        // `code` is the specific one on OpenAI (`invalid_api_key`); `type` on Anthropic.
        let kind = str_of("code").filter(|c| c.parse::<u16>().is_err()).or_else(|| str_of("type"));
        let request_id = v.get("request_id").and_then(Value::as_str).map(str::to_owned);
        Self { message, kind, request_id, body: v.to_string(), ..Self::default() }
    }

    /// A body that did not parse as the response we expected.
    pub(crate) fn invalid_response(err: &serde_json::Error, body: &[u8]) -> Self {
        Self {
            kind: Some("invalid_response".into()),
            message: format!("unexpected response: {err}"),
            body: String::from_utf8_lossy(&body[..body.len().min(8192)]).into_owned(),
            ..Self::default()
        }
    }

    /// Keeps the headers worth showing when debugging.
    pub(crate) fn take_headers(&mut self, h: &HeaderMap) {
        for (k, v) in h {
            let k = k.as_str();
            let keep = k.contains("ratelimit")
                || k.contains("request-id")
                || k.contains("requestid")
                || matches!(k, "retry-after" | "retry-after-ms" | "location" | "cf-ray" | "x-should-retry");
            if keep {
                self.headers.push((k.to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()));
            }
        }
        if self.request_id.is_none() {
            self.request_id = request_id(h);
        }
    }
}

/// Request id from the usual response headers.
pub(crate) fn request_id(h: &HeaderMap) -> Option<String> {
    ["x-request-id", "request-id", "x-amzn-requestid", "x-amz-request-id", "x-ms-request-id"]
        .iter()
        .find_map(|n| h.get(*n)?.to_str().ok())
        .map(str::to_owned)
}

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Error::Api(Box::new(e))
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

/// Best-effort human message from a JSON error object.
#[cfg(feature = "mcp")]
pub(crate) fn value_message(v: &Value) -> String {
    ApiError::from_value(v).message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_error_shapes() {
        let e = ApiError::from_body(
            401,
            br#"{"error":{"message":"bad key","type":"invalid_request_error","code":"invalid_api_key"}}"#,
        );
        assert_eq!((e.kind.as_deref(), e.message.as_str()), (Some("invalid_api_key"), "bad key"));
        let e = ApiError::from_body(
            529,
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_1"}"#,
        );
        assert_eq!((e.kind.as_deref(), e.request_id.as_deref()), (Some("overloaded_error"), Some("req_1")));
        assert!(Error::from(e).is_retryable());
        // MiniMax style: numeric code, generic type.
        let e = ApiError::from_body(401, br#"{"error":{"code":401,"message":"login fail","type":"api_error"}}"#);
        assert_eq!(e.kind.as_deref(), Some("api_error"));
        let e = ApiError::from_body(404, b"404 page not found");
        assert_eq!(e.message, "404 page not found");
        let s =
            Error::from(ApiError { url: "https://x/v1/chat/completions".into(), provider: "custom", ..e }).to_string();
        assert!(
            s.starts_with("custom error 404: 404 page not found (https://x/v1/chat/completions); check the base URL"),
            "{s}"
        );
        let e = ApiError { status: 400, kind: Some("invalid_request_error".into()), ..ApiError::default() };
        assert!(!Error::from(e).is_retryable());
    }
}
