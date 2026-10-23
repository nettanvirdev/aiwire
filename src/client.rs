use std::{
    borrow::Borrow,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use reqwest::{
    Url,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER},
    redirect,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    ChatStream, EmbedRequest, Embeddings, Error, Protocol, Provider, Request, Response, Result, anthropic, embed,
    error::{self, ApiError},
    openai, responses,
    stream::{Decoder, Origin},
};

/// Sent on the Anthropic protocol. The newest version drops sampling parameters; override with
/// [`ClientBuilder::header`] if you want it.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// An LLM API client. Cheap to clone; clones share one connection pool and concurrency limit.
#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    protocol: Protocol,
    provider: &'static str,
    chat_url: Url,
    chat_headers: HeaderMap,
    embed: Option<(Url, HeaderMap)>,
    max_tokens_field: &'static str,
    send_reasoning: bool,
    max_retries: u32,
    timeout: Duration,
    stream_idle: Duration,
    limit: Option<Arc<Semaphore>>,
}

/// A successful HTTP response and what it took to get it.
struct Sent {
    res: reqwest::Response,
    request_id: Option<String>,
    permit: Option<OwnedSemaphorePermit>,
    started: Instant,
}

impl Client {
    /// Client for a provider, using its default protocol.
    ///
    /// # Panics
    /// If the TLS backend fails to initialize, like [`reqwest::Client::new`].
    pub fn new(provider: Provider, api_key: impl Into<String>) -> Self {
        Self::builder().provider(provider).api_key(api_key).build().expect("client config is valid")
    }

    /// Client for a provider, reading the key from its env var (e.g. `GROQ_API_KEY`).
    pub fn from_env(provider: Provider) -> Result<Self> {
        Self::builder().provider(provider).build()
    }

    /// Full configuration.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Protocol this client speaks.
    pub fn protocol(&self) -> Protocol {
        self.inner.protocol
    }

    /// Send a chat request and wait for the whole answer.
    pub async fn chat(&self, req: impl Borrow<Request>) -> Result<Response> {
        let i = &*self.inner;
        let body = self.chat_body(req.borrow(), false)?;
        let sent = self.send(&i.chat_url, &i.chat_headers, body, false).await?;
        let bytes = sent.res.bytes().await?;
        let parsed = match i.protocol {
            Protocol::OpenAi => openai::parse_chat(&bytes),
            Protocol::Anthropic => anthropic::parse_chat(&bytes),
            Protocol::Responses => responses::parse_chat(&bytes),
        };
        let mut res = parsed.map_err(|e| self.response_error(e, &bytes, &i.chat_url, &sent.request_id))?;
        res.request_id = sent.request_id;
        #[cfg(feature = "tracing")]
        tracing::debug!(
            provider = i.provider,
            request_id = res.request_id.as_deref(),
            input_tokens = res.usage.input_tokens,
            output_tokens = res.usage.output_tokens,
            cached_tokens = res.usage.cached_tokens,
            finish_reason = ?res.finish_reason,
            elapsed_ms = sent.started.elapsed().as_millis() as u64,
            "aiwire chat finished"
        );
        drop(sent.permit);
        Ok(res)
    }

    /// Send a chat request and stream the answer.
    pub async fn stream(&self, req: impl Borrow<Request>) -> Result<ChatStream> {
        let i = &*self.inner;
        let body = self.chat_body(req.borrow(), true)?;
        let sent = self.send(&i.chat_url, &i.chat_headers, body, true).await?;
        let decoder = match i.protocol {
            Protocol::OpenAi => Decoder::OpenAi(Default::default()),
            Protocol::Anthropic => Decoder::Anthropic(Default::default()),
            Protocol::Responses => Decoder::Responses(Default::default()),
        };
        let origin = Origin {
            provider: i.provider,
            url: i.chat_url.to_string(),
            request_id: sent.request_id,
            started: sent.started,
        };
        Ok(ChatStream::new(sent.res, decoder, i.stream_idle, origin, sent.permit))
    }

    /// Embed texts. Needs a provider with an OpenAI-compatible `/embeddings` endpoint.
    pub async fn embed(&self, req: impl Borrow<EmbedRequest>) -> Result<Embeddings> {
        let (url, headers) = self
            .inner
            .embed
            .as_ref()
            .ok_or_else(|| Error::Config("this provider has no embeddings endpoint".into()))?;
        let body = serde_json::to_vec(&embed::Body(req.borrow()))?;
        let sent = self.send(url, headers, body.into(), false).await?;
        let bytes = sent.res.bytes().await?;
        embed::parse(&bytes).map_err(|e| self.response_error(e, &bytes, url, &sent.request_id))
    }

    fn chat_body(&self, req: &Request, stream: bool) -> Result<Bytes> {
        let i = &*self.inner;
        let body = match i.protocol {
            Protocol::OpenAi => serde_json::to_vec(&openai::ChatBody {
                req,
                stream,
                max_tokens_field: i.max_tokens_field,
                send_reasoning: i.send_reasoning,
            }),
            Protocol::Anthropic => serde_json::to_vec(&anthropic::ChatBody { req, stream }),
            Protocol::Responses => serde_json::to_vec(&responses::ChatBody { req, stream }),
        };
        // Serializing our own types only fails on content the protocol cannot express.
        Ok(body.map_err(|e| Error::Config(format!("request: {e}")))?.into())
    }

    /// Attaches call details to an error from parsing a 2xx body.
    fn response_error(&self, e: Error, body: &[u8], url: &Url, request_id: &Option<String>) -> Error {
        let mut api = match e {
            Error::Api(a) => *a,
            Error::Json(j) => ApiError::invalid_response(&j, body),
            e => return e,
        };
        api.status = 200;
        api.provider = self.inner.provider;
        api.url = url.to_string();
        api.attempts = 1;
        if api.request_id.is_none() {
            api.request_id.clone_from(request_id);
        }
        Error::Api(Box::new(api))
    }

    /// POST with retries on connection errors, timeouts, 408, 409, 429 and 5xx.
    async fn send(&self, url: &Url, headers: &HeaderMap, body: Bytes, stream: bool) -> Result<Sent> {
        let i = &*self.inner;
        let permit = match &i.limit {
            Some(s) => Some(s.clone().acquire_owned().await.expect("semaphore is never closed")),
            None => None,
        };
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut rb = i
                .http
                .post(url.clone())
                .headers(headers.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
            if stream {
                rb = rb.header(ACCEPT, "text/event-stream");
            } else {
                rb = rb.timeout(i.timeout);
            }
            #[cfg(feature = "tracing")]
            tracing::debug!(provider = i.provider, %url, attempt, bytes = body.len(), stream, "aiwire request");
            #[cfg(feature = "tracing")]
            tracing::trace!(body = %String::from_utf8_lossy(&body), "aiwire request body");
            let sent = if stream {
                // Streams have no whole-request timeout, but the headers must come back.
                match tokio::time::timeout(i.stream_idle, rb.send()).await {
                    Ok(r) => r.map_err(Error::from),
                    Err(_) => Err(Error::Timeout(i.stream_idle)),
                }
            } else {
                rb.send().await.map_err(Error::from)
            };
            let retryable = attempt <= i.max_retries;
            let (wait, why) = match sent {
                Ok(r) if r.status().is_success() => {
                    let request_id = error::request_id(r.headers());
                    #[cfg(feature = "tracing")]
                    tracing::debug!(
                        provider = i.provider,
                        status = r.status().as_u16(),
                        request_id = request_id.as_deref(),
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "aiwire response"
                    );
                    return Ok(Sent { res: r, request_id, permit, started });
                }
                Ok(r) => {
                    let status = r.status().as_u16();
                    let should_retry = match r.headers().get("x-should-retry").map(|v| v.as_bytes()) {
                        Some(b"true") => true,
                        Some(b"false") => false,
                        _ => matches!(status, 408 | 409 | 429 | 500..=599),
                    };
                    let wait = retry_after(r.headers());
                    if !(retryable && should_retry) {
                        let headers = r.headers().clone();
                        let body = r.bytes().await.unwrap_or_default();
                        let mut e = ApiError::from_body(status, &body);
                        e.take_headers(&headers);
                        e.provider = i.provider;
                        e.url = url.to_string();
                        e.retry_after = wait;
                        e.attempts = attempt;
                        #[cfg(feature = "tracing")]
                        tracing::warn!(error = %e, "aiwire request failed");
                        return Err(e.into());
                    }
                    (wait, format!("HTTP {status}"))
                }
                Err(e) if retryable && is_transient(&e) => (None, e.to_string()),
                Err(e) => return Err(e),
            };
            let wait = wait.unwrap_or_else(|| backoff(attempt - 1));
            #[cfg(feature = "tracing")]
            tracing::warn!(provider = i.provider, attempt, wait_ms = wait.as_millis() as u64, reason = %why, "aiwire retrying");
            #[cfg(not(feature = "tracing"))]
            let _ = why;
            tokio::time::sleep(wait).await;
        }
    }
}

/// Failures worth retrying before any response arrived.
fn is_transient(e: &Error) -> bool {
    match e {
        Error::Http(e) => e.is_connect() || e.is_timeout() || e.is_request(),
        Error::Timeout(_) => true,
        _ => false,
    }
}

/// 0.5s, 1s, 2s, 4s, 8s … with ±12% jitter.
fn backoff(attempt: u32) -> Duration {
    let base = 500u64 << attempt.min(4);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::from(d.subsec_nanos()));
    Duration::from_millis(base - base / 8 + nanos % (base / 4 + 1))
}

fn retry_after(h: &HeaderMap) -> Option<Duration> {
    let num = |name: &str| h.get(name)?.to_str().ok()?.trim().parse::<f64>().ok();
    let secs = num("retry-after-ms").map(|ms| ms / 1000.0).or_else(|| num(RETRY_AFTER.as_str()))?;
    (0.0..=60.0).contains(&secs).then(|| Duration::from_secs_f64(secs))
}

/// Configures a [`Client`]. Set a [`provider`](Self::provider), a [`base_url`](Self::base_url), or both.
#[derive(Default)]
pub struct ClientBuilder {
    provider: Option<Provider>,
    protocol: Option<Protocol>,
    base_url: Option<String>,
    api_key: Option<String>,
    headers: Vec<(String, String)>,
    http: Option<reqwest::Client>,
    max_retries: Option<u32>,
    timeout: Option<Duration>,
    stream_idle: Option<Duration>,
    max_concurrent: Option<usize>,
}

impl ClientBuilder {
    /// Known provider: fills in URL, auth style and key env var.
    pub fn provider(mut self, provider: Provider) -> Self {
        self.provider = Some(provider);
        self
    }
    /// Wire format. Defaults to the provider's preferred one, or OpenAI for a bare `base_url`.
    /// Building fails if the provider does not speak it.
    pub fn protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = Some(protocol);
        self
    }
    /// API root, e.g. `http://localhost:8000/v1`. Overrides the provider URL (proxies, gateways).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }
    /// API key. Defaults to the provider's env var.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
    /// Extra header on every request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    /// Bring your own `reqwest::Client` (proxies, custom TLS, shared pool).
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }
    /// Retries on connection errors, timeouts, 408, 409, 429 and 5xx. Default 2.
    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = Some(n);
        self
    }
    /// Whole-request timeout for non-streaming calls. Default 10 minutes.
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
    /// Streams fail with [`Error::Timeout`] when the server sends nothing for this long,
    /// including while waiting for the response headers. Default 5 minutes.
    pub fn stream_idle_timeout(mut self, t: Duration) -> Self {
        self.stream_idle = Some(t);
        self
    }
    /// At most `n` requests in flight across all clones of this client; the rest wait their
    /// turn. A stream holds its slot until it ends or is dropped. Default: no limit.
    pub fn max_concurrent_requests(mut self, n: usize) -> Self {
        self.max_concurrent = Some(n.max(1));
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<Client> {
        let cfg = |m: String| Error::Config(m);
        let protocol = self.protocol.or(self.provider.map(|p| p.protocol)).unwrap_or(Protocol::OpenAi);
        if let Some(p) = self.provider.filter(|p| !p.supports(protocol)) {
            return Err(cfg(format!("{} does not offer the {protocol:?} protocol", p.name)));
        }
        let base = match (&self.base_url, self.provider) {
            (Some(u), _) => u.as_str(),
            (None, Some(p)) => p.url(protocol).unwrap_or_default(),
            (None, None) => return Err(cfg("set a provider or a base_url".into())),
        };
        let base = base.trim_end_matches('/');

        let env = self.provider.map_or("", |p| p.api_key_env);
        let key = self.api_key.or_else(|| std::env::var(env).ok().filter(|_| !env.is_empty()));
        if key.is_none() && !env.is_empty() {
            return Err(cfg(format!("missing API key: call .api_key() or set {env}")));
        }

        let mut extra = HeaderMap::new();
        for (k, v) in &self.headers {
            let name = HeaderName::try_from(k.as_str()).map_err(|e| cfg(format!("header {k}: {e}")))?;
            let value = HeaderValue::try_from(v.as_str()).map_err(|e| cfg(format!("header {k}: {e}")))?;
            extra.insert(name, value);
        }
        let bearer = |headers: &mut HeaderMap| -> Result<()> {
            if let Some(k) = &key {
                headers.insert(AUTHORIZATION, secret(&format!("Bearer {k}"))?);
            }
            Ok(())
        };
        let openai_headers = || -> Result<HeaderMap> {
            let mut h = HeaderMap::new();
            bearer(&mut h)?;
            h.extend(extra.clone());
            Ok(h)
        };
        let url =
            |path: &str| Url::parse(&format!("{base}{path}")).map_err(|e| cfg(format!("bad base url {base}: {e}")));

        let (chat_url, chat_headers) = match protocol {
            Protocol::OpenAi => (url("/chat/completions")?, openai_headers()?),
            Protocol::Responses => (url("/responses")?, openai_headers()?),
            Protocol::Anthropic => {
                let mut h = HeaderMap::new();
                h.insert("anthropic-version", HeaderValue::from_static(ANTHROPIC_VERSION));
                // Unknown gateways get both auth styles; known providers get the one they document.
                let (x_api_key, bearer_auth) = match self.provider {
                    Some(p) => (!p.anthropic_bearer, p.anthropic_bearer),
                    None => (true, true),
                };
                if let (true, Some(k)) = (x_api_key, &key) {
                    h.insert("x-api-key", secret(k)?);
                }
                if bearer_auth {
                    bearer(&mut h)?;
                }
                h.extend(extra.clone());
                (url("/v1/messages")?, h)
            }
        };

        let embed = match (self.provider, protocol) {
            (Some(p), _) if !p.embeddings => None,
            (_, Protocol::OpenAi | Protocol::Responses) => Some((url("/embeddings")?, chat_headers.clone())),
            (Some(p), Protocol::Anthropic) => {
                let base = p.openai_url.unwrap_or_default().trim_end_matches('/');
                let u = Url::parse(&format!("{base}/embeddings")).map_err(|e| cfg(e.to_string()))?;
                Some((u, openai_headers()?))
            }
            (None, Protocol::Anthropic) => None,
        };

        let http = match self.http {
            Some(h) => h,
            None => reqwest::Client::builder()
                .user_agent(concat!("aiwire/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(10))
                .pool_idle_timeout(Duration::from_secs(90))
                .tcp_nodelay(true)
                .tcp_keepalive(Duration::from_secs(30))
                .http2_adaptive_window(true)
                // 301/302 turn a POST into a GET; report them instead of following.
                .redirect(redirect::Policy::custom(|a| {
                    if matches!(a.status().as_u16(), 307 | 308) && a.previous().len() < 5 {
                        a.follow()
                    } else {
                        a.stop()
                    }
                }))
                .build()?,
        };

        Ok(Client {
            inner: Arc::new(Inner {
                http,
                protocol,
                provider: self.provider.map_or("custom", |p| p.name),
                chat_url,
                chat_headers,
                embed,
                max_tokens_field: self.provider.map_or("max_tokens", |p| p.max_tokens_field),
                send_reasoning: self.provider.is_some_and(|p| p.send_reasoning),
                max_retries: self.max_retries.unwrap_or(2),
                timeout: self.timeout.unwrap_or(Duration::from_secs(600)),
                stream_idle: self.stream_idle.unwrap_or(Duration::from_secs(300)),
                limit: self.max_concurrent.map(|n| Arc::new(Semaphore::new(n))),
            }),
        })
    }
}

fn secret(v: &str) -> Result<HeaderValue> {
    let mut h = HeaderValue::try_from(v).map_err(|_| Error::Config("API key has invalid characters".into()))?;
    h.set_sensitive(true);
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_must_match_provider() {
        let e = Client::builder().provider(Provider::GROQ).protocol(Protocol::Anthropic).api_key("k").build();
        assert!(matches!(e, Err(Error::Config(m)) if m.contains("groq")));
        let c =
            Client::builder().provider(Provider::MINIMAX).protocol(Protocol::Anthropic).api_key("k").build().unwrap();
        assert_eq!(c.inner.chat_url.as_str(), "https://api.minimax.io/anthropic/v1/messages");
        assert!(c.inner.chat_headers.contains_key(AUTHORIZATION));
        assert_eq!(c.inner.embed, None);
        let c = Client::new(Provider::ANTHROPIC, "k");
        assert_eq!(c.protocol(), Protocol::Anthropic);
        assert!(c.inner.chat_headers.contains_key("x-api-key") && !c.inner.chat_headers.contains_key(AUTHORIZATION));
        let c = Client::builder().provider(Provider::ANTHROPIC).api_key("k").header("anthropic-version", "v2").build();
        let h = &c.unwrap().inner.chat_headers;
        assert_eq!(h.get_all("anthropic-version").iter().collect::<Vec<_>>(), ["v2"]);
        let c = Client::builder().provider(Provider::OPENAI).protocol(Protocol::Responses).api_key("k").build();
        assert_eq!(c.unwrap().inner.chat_url.as_str(), "https://api.openai.com/v1/responses");
        let e = Client::builder().provider(Provider::DEEPSEEK).protocol(Protocol::Responses).api_key("k").build();
        assert!(e.is_err());
        assert!(Client::new(Provider::DEEPSEEK, "k").inner.send_reasoning);
    }

    #[test]
    fn missing_key_names_env_var() {
        let e = Client::builder().provider(Provider { api_key_env: "AIWIRE_TEST_UNSET", ..Provider::GROQ }).build();
        assert!(matches!(e, Err(Error::Config(m)) if m.contains("AIWIRE_TEST_UNSET")));
        assert!(Client::from_env(Provider::OLLAMA).is_ok());
    }

    #[test]
    fn retry_after_header() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("2"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(2)));
        h.insert("retry-after-ms", HeaderValue::from_static("250"));
        assert_eq!(retry_after(&h), Some(Duration::from_millis(250)));
        assert!(backoff(10) <= Duration::from_millis(9000));
    }
}
