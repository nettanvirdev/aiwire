//! OAuth 2.1 for remote MCP servers: discovery, dynamic client registration, PKCE login,
//! refresh. See [`Mcp::http_oauth`](super::Mcp::http_oauth).
//!
//! ```no_run
//! # async fn run() -> aiwire::Result<()> {
//! use aiwire::mcp::{Mcp, OAuth};
//!
//! // Opens the browser for the login and catches the redirect on 127.0.0.1:8765.
//! let mcp = Mcp::http_oauth("https://mcp.example.com/mcp", OAuth::new("http://127.0.0.1:8765/callback")).await?;
//! let tools = mcp.tools().await?;
//! let saved = mcp.tokens().await; // store these (they are secrets) and pass them back with OAuth::tokens
//! # Ok(()) }
//! ```

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{
    Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

use crate::{Error, Result, error::ApiError, types::URL_SAFE, types::base64_into};

type Authorize = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync>;

/// OAuth settings for [`Mcp::http_oauth`](super::Mcp::http_oauth).
#[derive(Clone)]
pub struct OAuth {
    redirect_uri: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    scope: Option<String>,
    client_name: String,
    tokens: Option<Tokens>,
    authorize: Option<Authorize>,
}

impl fmt::Debug for OAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth")
            .field("redirect_uri", &self.redirect_uri)
            .field("client_id", &self.client_id)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl OAuth {
    /// Login with this redirect URI. With the default browser login it must be a loopback
    /// address (`http://127.0.0.1:<port>/<path>` or `http://localhost:…`) that this process
    /// listens on during the login.
    pub fn new(redirect_uri: impl Into<String>) -> Self {
        Self {
            redirect_uri: redirect_uri.into(),
            client_id: None,
            client_secret: None,
            scope: None,
            client_name: "aiwire".into(),
            tokens: None,
            authorize: None,
        }
    }
    /// A client registered with the authorization server beforehand. Without one, the client
    /// registers itself (dynamic client registration).
    pub fn client(mut self, id: impl Into<String>, secret: Option<String>) -> Self {
        self.client_id = Some(id.into());
        self.client_secret = secret;
        self
    }
    /// Scopes to ask for, space separated. Default: what the server advertises.
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }
    /// Name shown on the consent screen when registering. Default `aiwire`.
    pub fn client_name(mut self, name: impl Into<String>) -> Self {
        self.client_name = name.into();
        self
    }
    /// Tokens saved from an earlier [`Mcp::tokens`](super::Mcp::tokens): skips the login while
    /// they (or their refresh token) are valid.
    pub fn tokens(mut self, tokens: Tokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Run the login yourself: `f` gets the authorization URL to show the user and returns the
    /// URL the browser was redirected to (or just the `code`). Default: open the system browser
    /// and catch the redirect on the loopback redirect URI.
    pub fn on_authorize<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String>> + Send + 'static,
    {
        self.authorize = Some(Arc::new(move |url| Box::pin(f(url))));
        self
    }
}

/// OAuth tokens plus what is needed to refresh them. Secrets: store them like passwords.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    /// Bearer token sent to the MCP server.
    pub access_token: String,
    /// Token to get a new access token without a login.
    pub refresh_token: Option<String>,
    /// Unix time (seconds) the access token expires.
    pub expires_at: Option<u64>,
    /// Token endpoint used for refreshing.
    pub token_endpoint: String,
    /// Client the tokens were issued to.
    pub client_id: String,
    /// Client secret, for confidential clients.
    pub client_secret: Option<String>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

pub(super) fn bearer(token: &str) -> Result<HeaderValue> {
    let mut v = HeaderValue::try_from(format!("Bearer {token}")).map_err(|e| Error::Mcp(e.to_string()))?;
    v.set_sensitive(true);
    Ok(v)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn oauth_err(e: impl fmt::Display) -> Error {
    Error::Mcp(format!("oauth: {e}"))
}

/// Live OAuth state of one connection.
pub(super) struct Auth {
    cfg: OAuth,
    tokens: Mutex<Option<Tokens>>,
}

impl Auth {
    pub fn new(mut cfg: OAuth) -> Self {
        let tokens = cfg.tokens.take();
        Self { cfg, tokens: Mutex::new(tokens) }
    }

    pub async fn tokens(&self) -> Option<Tokens> {
        self.tokens.lock().await.clone()
    }

    /// Authorization header for the first request: saved tokens (refreshed if expired), or a
    /// login when the server asks for one.
    pub async fn header(&self, http: &reqwest::Client, url: &Url) -> Result<Option<HeaderValue>> {
        let mut t = self.tokens.lock().await;
        if let Some(tok) = t.as_ref() {
            if tok.expires_at.is_some_and(|e| e <= now() + 30) {
                *t = self.refresh(http, url, tok).await.ok();
            }
        }
        match t.as_ref() {
            Some(tok) => Ok(Some(bearer(&tok.access_token)?)),
            None => Ok(None), // the server's 401 starts the login, with its metadata hint
        }
    }

    /// After a 401: refresh, or log in again. Returns the new Authorization header.
    pub async fn reauthorize(&self, http: &reqwest::Client, url: &Url, challenge: Option<&str>) -> Result<HeaderValue> {
        let mut t = self.tokens.lock().await;
        let refreshed = match t.as_ref() {
            Some(tok) if tok.refresh_token.is_some() => self.refresh(http, url, tok).await.ok(),
            _ => None,
        };
        let tok = match refreshed {
            Some(tok) => tok,
            None => self.login(http, url, challenge).await?,
        };
        let v = bearer(&tok.access_token)?;
        *t = Some(tok);
        Ok(v)
    }

    async fn refresh(&self, http: &reqwest::Client, url: &Url, tok: &Tokens) -> Result<Tokens> {
        let refresh = tok.refresh_token.as_deref().ok_or_else(|| oauth_err("no refresh token"))?;
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", &tok.client_id),
            ("resource", url.as_str()),
        ];
        if let Some(s) = &tok.client_secret {
            form.push(("client_secret", s));
        }
        let mut new = token_request(http, &tok.token_endpoint, &form).await?;
        new.refresh_token = new.refresh_token.or_else(|| tok.refresh_token.clone());
        Ok(Tokens { client_id: tok.client_id.clone(), client_secret: tok.client_secret.clone(), ..new })
    }

    /// Discovery, registration, PKCE authorization code flow.
    async fn login(&self, http: &reqwest::Client, url: &Url, challenge: Option<&str>) -> Result<Tokens> {
        let cfg = &self.cfg;
        let resource = resource_metadata(http, url, challenge).await;
        let issuer = resource
            .as_ref()
            .and_then(|r| r["authorization_servers"][0].as_str())
            .and_then(|s| Url::parse(s).ok())
            .unwrap_or_else(|| origin(url));
        let meta = server_metadata(http, &issuer).await;
        let endpoint = |key: &str, path: &str| {
            meta.as_ref()
                .and_then(|m| m[key].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| origin(&issuer).join(path).map(String::from).unwrap_or_default())
        };
        let (authorization_endpoint, token_endpoint) =
            (endpoint("authorization_endpoint", "/authorize"), endpoint("token_endpoint", "/token"));
        let scope = cfg.scope.clone().or_else(|| {
            let s = challenge.and_then(|c| param(c, "scope"));
            s.or_else(|| {
                let list = resource.as_ref()?["scopes_supported"].as_array()?;
                Some(list.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
            })
            .filter(|s| !s.is_empty())
        });

        let (client_id, client_secret) = match &cfg.client_id {
            Some(id) => (id.clone(), cfg.client_secret.clone()),
            None => {
                let reg = meta
                    .as_ref()
                    .and_then(|m| m["registration_endpoint"].as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| oauth_err("server has no registration endpoint; pass OAuth::client"))?;
                register(http, &reg, cfg).await?
            }
        };

        let verifier = random_token();
        let mut challenge_bytes = String::new();
        base64_into(&sha256(verifier.as_bytes()), &mut challenge_bytes, URL_SAFE);
        let state = random_token();
        let mut auth_url = Url::parse(&authorization_endpoint).map_err(oauth_err)?;
        {
            let mut q = auth_url.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", &client_id)
                .append_pair("redirect_uri", &cfg.redirect_uri)
                .append_pair("code_challenge", &challenge_bytes)
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", &state)
                .append_pair("resource", url.as_str());
            if let Some(s) = &scope {
                q.append_pair("scope", s);
            }
        }
        let landed = match &cfg.authorize {
            Some(f) => f(auth_url.to_string()).await?,
            None => browser_login(auth_url.as_str(), &cfg.redirect_uri).await?,
        };
        let code = match Url::parse(&landed) {
            Ok(u) => {
                let get = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
                if let Some(e) = get("error") {
                    return Err(oauth_err(format!(
                        "login failed: {e} {}",
                        get("error_description").unwrap_or_default()
                    )));
                }
                if get("state").as_deref() != Some(state.as_str()) {
                    return Err(oauth_err("state mismatch in the redirect; login aborted"));
                }
                get("code").ok_or_else(|| oauth_err("no code in the redirect"))?
            }
            Err(_) => landed, // the caller returned the bare code
        };

        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", cfg.redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
            ("resource", url.as_str()),
        ];
        if let Some(s) = &client_secret {
            form.push(("client_secret", s));
        }
        let tok = token_request(http, &token_endpoint, &form).await?;
        Ok(Tokens { client_id, client_secret, ..tok })
    }
}

fn origin(u: &Url) -> Url {
    let mut o = u.clone();
    o.set_path("/");
    o.set_query(None);
    o.set_fragment(None);
    o
}

/// `key="value"` from a `WWW-Authenticate` header.
fn param(header: &str, key: &str) -> Option<String> {
    let at = header.find(&format!("{key}="))? + key.len() + 1;
    let rest = &header[at..];
    let v = match rest.strip_prefix('"') {
        Some(q) => &q[..q.find('"')?],
        None => rest.split([',', ' ']).next()?,
    };
    Some(v.to_owned())
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let res = http.get(url).header(ACCEPT, "application/json").send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    serde_json::from_slice(&res.bytes().await.ok()?).ok()
}

/// RFC 9728 protected resource metadata: from the 401 hint, else the well-known paths.
async fn resource_metadata(http: &reqwest::Client, url: &Url, challenge: Option<&str>) -> Option<Value> {
    if let Some(u) = challenge.and_then(|c| param(c, "resource_metadata")) {
        if let Some(v) = get_json(http, &u).await {
            return Some(v);
        }
    }
    let path = url.path().trim_end_matches('/');
    for p in [format!("/.well-known/oauth-protected-resource{path}"), "/.well-known/oauth-protected-resource".into()] {
        if let Some(v) = get_json(http, origin(url).join(&p).ok()?.as_str()).await {
            return Some(v);
        }
    }
    None
}

/// RFC 8414 / OpenID Connect discovery for the authorization server.
async fn server_metadata(http: &reqwest::Client, issuer: &Url) -> Option<Value> {
    let path = issuer.path().trim_end_matches('/');
    let candidates = [
        format!("/.well-known/oauth-authorization-server{path}"),
        format!("/.well-known/openid-configuration{path}"),
        format!("{path}/.well-known/openid-configuration"),
    ];
    for p in candidates {
        if let Some(v) = get_json(http, origin(issuer).join(&p).ok()?.as_str()).await {
            return Some(v);
        }
    }
    None
}

async fn register(http: &reqwest::Client, endpoint: &str, cfg: &OAuth) -> Result<(String, Option<String>)> {
    let body = json!({
        "client_name": cfg.client_name,
        "redirect_uris": [cfg.redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let res = http.post(endpoint).header(CONTENT_TYPE, "application/json").body(body.to_string()).send().await?;
    let v = json_or_error(res, endpoint).await?;
    let id = v["client_id"].as_str().ok_or_else(|| oauth_err("registration returned no client_id"))?;
    Ok((id.to_owned(), v["client_secret"].as_str().map(str::to_owned)))
}

async fn token_request(http: &reqwest::Client, endpoint: &str, form: &[(&str, &str)]) -> Result<Tokens> {
    let mut enc = Url::parse("http://x/").expect("valid");
    enc.query_pairs_mut().extend_pairs(form);
    let res = http
        .post(endpoint)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(ACCEPT, "application/json")
        .body(enc.query().unwrap_or_default().to_owned())
        .send()
        .await?;
    let v = json_or_error(res, endpoint).await?;
    Ok(Tokens {
        access_token: v["access_token"]
            .as_str()
            .ok_or_else(|| oauth_err("token response has no access_token"))?
            .to_owned(),
        refresh_token: v["refresh_token"].as_str().map(str::to_owned),
        expires_at: v["expires_in"].as_u64().map(|s| now() + s),
        token_endpoint: endpoint.to_owned(),
        client_id: String::new(),
        client_secret: None,
    })
}

async fn json_or_error(res: reqwest::Response, url: &str) -> Result<Value> {
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    let body = res.bytes().await?;
    if !(200..300).contains(&status) {
        let mut e = ApiError::from_body(status, &body);
        e.take_headers(&headers);
        e.provider = "oauth";
        e.url = url.to_owned();
        e.attempts = 1;
        if let Some(d) =
            serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["error_description"].as_str().map(str::to_owned))
        {
            e.message = d;
        }
        return Err(e.into());
    }
    serde_json::from_slice(&body).map_err(|e| oauth_err(format!("{url}: {e}")))
}

/// 32 random bytes, base64url: a PKCE verifier or a `state`.
fn random_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("the OS random number generator works");
    let mut s = String::new();
    base64_into(&b, &mut s, URL_SAFE);
    s
}

/// Opens `auth_url` in the system browser and waits (up to 5 minutes) for the redirect to the
/// loopback `redirect_uri`. Returns the full URL the browser landed on.
pub async fn browser_login(auth_url: &str, redirect_uri: &str) -> Result<String> {
    let redirect = Url::parse(redirect_uri).map_err(oauth_err)?;
    let host = redirect.host_str().unwrap_or_default();
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return Err(oauth_err("the browser login needs a loopback redirect URI; use OAuth::on_authorize otherwise"));
    }
    let port = redirect.port().ok_or_else(|| oauth_err("the redirect URI needs an explicit port"))?;
    let listener = TcpListener::bind((if host == "[::1]" { "::1" } else { "127.0.0.1" }, port))
        .await
        .map_err(|e| oauth_err(format!("listen on {redirect_uri}: {e}")))?;
    eprintln!("Sign in to the MCP server in your browser. If it did not open, visit:\n{auth_url}");
    open_browser(auth_url);
    let wait = async {
        loop {
            let (mut sock, _) = listener.accept().await.map_err(oauth_err)?;
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.map_err(oauth_err)?;
            let head = String::from_utf8_lossy(&buf[..n]);
            let target = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
            if !target.starts_with(redirect.path()) {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n").await;
                continue;
            }
            let page = "<html><body><h3>Signed in. You can close this tab.</h3></body></html>";
            let res =
                format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n{page}", page.len());
            let _ = sock.write_all(res.as_bytes()).await;
            return Ok(format!("{}://{}:{port}{target}", redirect.scheme(), host));
        }
    };
    tokio::time::timeout(Duration::from_secs(300), wait).await.map_err(|_| oauth_err("login timed out"))?
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    let cmd = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    let _ = cmd; // the URL is printed too, in case no browser opens
}

/// SHA-256 (FIPS 180-4), for the PKCE code challenge.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    let mut out = [0u8; 32];
    for (o, x) in out.chunks_exact_mut(4).zip(h) {
        o.copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// PKCE S256 challenge of a verifier; public for tests.
#[doc(hidden)]
pub fn pkce_challenge(verifier: &str) -> String {
    let mut s = String::new();
    base64_into(&sha256(verifier.as_bytes()), &mut s, URL_SAFE);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(sha256(long)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 43);
    }

    #[test]
    fn www_authenticate() {
        let h = r#"Bearer error="invalid_token", resource_metadata="https://x/.well-known/oauth-protected-resource", scope="a b""#;
        assert_eq!(param(h, "resource_metadata").as_deref(), Some("https://x/.well-known/oauth-protected-resource"));
        assert_eq!(param(h, "scope").as_deref(), Some("a b"));
        assert_eq!(param("Bearer realm=x, scope=y", "scope").as_deref(), Some("y"));
    }
}
