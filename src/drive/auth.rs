//! OAuth 2.0 for a personal Google Drive, with secrets in `.env`.
//!
//! `run` performs the one-time loopback consent flow (PKCE, `drive.file` scope)
//! and writes `GOOGLE_REFRESH_TOKEN` back into `.env`. `access_token` exchanges
//! that refresh token for a short-lived access token on each upload run.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

#[derive(Deserialize)]
struct TokenResponse {
    #[allow(dead_code)] // read by access_token(), which M3 (upload) will call
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Run the interactive consent flow (the `auth` command), writing the resulting
/// refresh token into `env_path`.
pub fn run(env_path: &Path) -> Result<()> {
    let client_id = require_env("GOOGLE_CLIENT_ID")?;
    let client_secret = require_env("GOOGLE_CLIENT_SECRET")?;

    // Loopback server on an OS-assigned ephemeral port (allowed for Desktop
    // OAuth clients without pre-registration).
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("starting loopback server: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .context("loopback server has no IP address")?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let verifier = random_token(64);
    let challenge = pkce_challenge(&verifier);
    let state = random_token(24);
    let auth_url = build_auth_url(&client_id, &redirect_uri, &challenge, &state);

    println!("Opening your browser to authorize Google Drive access…");
    println!("If it doesn't open, visit this URL manually:\n\n{auth_url}\n");
    let _ = webbrowser::open(&auth_url);

    let (code, returned_state) = wait_for_code(&server)?;
    if returned_state != state {
        bail!("OAuth state mismatch (possible CSRF) — aborting");
    }

    let tokens = exchange_code(&client_id, &client_secret, &code, &verifier, &redirect_uri)?;
    let refresh = tokens.refresh_token.context(
        "Google did not return a refresh token; revoke prior access for this app and retry",
    )?;

    upsert_env_file(env_path, "GOOGLE_REFRESH_TOKEN", &refresh)?;
    println!("✓ Authorized. Refresh token written to {}", env_path.display());
    Ok(())
}

/// Exchange the stored refresh token for a short-lived access token.
#[allow(dead_code)] // called by M3 (upload)
pub fn access_token() -> Result<String> {
    let client_id = require_env("GOOGLE_CLIENT_ID")?;
    let client_secret = require_env("GOOGLE_CLIENT_SECRET")?;
    let refresh = require_env("GOOGLE_REFRESH_TOKEN")
        .context("no GOOGLE_REFRESH_TOKEN — run `video-uploader auth` first")?;

    let resp = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .context("refresh-token request")?;
    Ok(parse_token_response(resp)?.access_token)
}

/// POST the authorization code to the token endpoint.
fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse> {
    let resp = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .context("authorization-code exchange")?;
    parse_token_response(resp)
}

fn parse_token_response(resp: reqwest::blocking::Response) -> Result<TokenResponse> {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("token endpoint returned {status}: {body}");
    }
    serde_json::from_str(&body).context("parsing token response")
}

/// Block on the loopback server until Google redirects back with the code.
fn wait_for_code(server: &tiny_http::Server) -> Result<(String, String)> {
    for request in server.incoming_requests() {
        let params = parse_query(request.url());
        if let Some(err) = params.get("error") {
            let _ = request.respond(html("Authorization failed. You can close this tab."));
            bail!("authorization denied: {err}");
        }
        match (params.get("code"), params.get("state")) {
            (Some(code), Some(state)) => {
                let (code, state) = (code.clone(), state.clone());
                let _ = request.respond(html("Authorization complete — you can close this tab."));
                return Ok((code, state));
            }
            _ => {
                let _ = request.respond(html("Waiting for authorization…"));
            }
        }
    }
    bail!("loopback server closed before receiving the authorization code");
}

fn html(message: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let body = format!("<!doctype html><meta charset=utf-8><body style=\"font-family:sans-serif\"><h2>{message}</h2>");
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("valid header");
    tiny_http::Response::from_string(body).with_header(header)
}

/// Read a required, non-empty environment variable.
fn require_env(key: &str) -> Result<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => bail!("missing {key} (set it in .env)"),
    }
}

/// PKCE S256 challenge: base64url(sha256(verifier)), no padding.
fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// A URL-safe random token from the system RNG (PKCE verifier / CSRF state).
fn random_token(len: usize) -> String {
    // 64 unreserved chars -> uniform mapping from a byte (256 % 64 == 0).
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut buf = vec![0u8; len];
    getrandom::getrandom(&mut buf).expect("system RNG unavailable");
    buf.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

/// Build the Google authorization URL with all required query params.
fn build_auth_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    url::Url::parse_with_params(
        AUTH_ENDPOINT,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ],
    )
    .expect("valid authorization URL")
    .to_string()
}

/// Parse the query string of a redirect path like `/?code=...&state=...`,
/// percent-decoding values.
fn parse_query(path_and_query: &str) -> HashMap<String, String> {
    match url::Url::parse(&format!("http://localhost{path_and_query}")) {
        Ok(u) => u
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect(),
        Err(_) => HashMap::new(),
    }
}

/// Write `KEY=value` into `.env`, replacing an existing line or appending.
fn upsert_env_file(path: &Path, key: &str, value: &str) -> Result<()> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let updated = upsert_env_line(&content, key, value);
    std::fs::write(path, updated).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Pure helper: insert or replace a `KEY=value` line in `.env` content.
fn upsert_env_line(content: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}=");
    let mut replaced = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.trim_start().starts_with(&prefix) {
                replaced = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !replaced {
        lines.push(format!("{key}={value}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(pkce_challenge(verifier), expected);
    }

    #[test]
    fn random_token_has_requested_length_and_charset() {
        let t = random_token(64);
        assert_eq!(t.chars().count(), 64);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        // Overwhelmingly likely to differ across calls.
        assert_ne!(random_token(64), random_token(64));
    }

    #[test]
    fn auth_url_contains_required_params() {
        let url = build_auth_url("cid", "http://127.0.0.1:9999", "chal", "st");
        let parsed = url::Url::parse(&url).unwrap();
        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "cid");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:9999");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["scope"], SCOPE);
        assert_eq!(q["access_type"], "offline");
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "st");
    }

    #[test]
    fn parse_query_decodes_pairs() {
        let q = parse_query("/?code=4%2Fabc&state=xyz");
        assert_eq!(q["code"], "4/abc"); // %2F decoded to '/'
        assert_eq!(q["state"], "xyz");
    }

    #[test]
    fn upsert_replaces_existing_key() {
        let content = "GOOGLE_CLIENT_ID=abc\nGOOGLE_REFRESH_TOKEN=\nDRIVE_ROOT_FOLDER=X\n";
        let out = upsert_env_line(content, "GOOGLE_REFRESH_TOKEN", "1//tok");
        assert!(out.contains("GOOGLE_REFRESH_TOKEN=1//tok"));
        assert!(out.contains("GOOGLE_CLIENT_ID=abc")); // others preserved
        assert!(out.contains("DRIVE_ROOT_FOLDER=X"));
        // exactly one refresh-token line
        assert_eq!(out.matches("GOOGLE_REFRESH_TOKEN=").count(), 1);
    }

    #[test]
    fn upsert_appends_missing_key() {
        let out = upsert_env_line("GOOGLE_CLIENT_ID=abc\n", "GOOGLE_REFRESH_TOKEN", "tok");
        assert!(out.contains("GOOGLE_CLIENT_ID=abc"));
        assert!(out.ends_with("GOOGLE_REFRESH_TOKEN=tok\n"));
    }

    #[test]
    fn upsert_does_not_match_prefix_collisions() {
        // A different key that merely starts with the same text must be left alone.
        let content = "GOOGLE_REFRESH_TOKEN_BACKUP=keep\n";
        let out = upsert_env_line(content, "GOOGLE_REFRESH_TOKEN", "new");
        assert!(out.contains("GOOGLE_REFRESH_TOKEN_BACKUP=keep"));
        assert!(out.contains("GOOGLE_REFRESH_TOKEN=new"));
    }
}
