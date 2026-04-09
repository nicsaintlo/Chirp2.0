/// Google OAuth 2.0 PKCE flow for native/desktop apps (RFC 8252).
///
/// Google explicitly supports PKCE for "Desktop app" type OAuth clients.
/// Register at: https://console.cloud.google.com
///   → APIs & Services → Credentials → Create → OAuth client ID → Desktop app
/// Enable: "Generative Language API" (for Gemini / Gemma access)
///
/// Set at build time:
///   GOOGLE_CLIENT_ID=<id>        (required)
///   GOOGLE_CLIENT_SECRET=<secret> (required — Google needs it for desktop apps)
///
/// Both are OK to embed in open-source code; Google explicitly acknowledges
/// that desktop apps cannot keep secrets truly secret (see OAuth docs).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::net::TcpListener;
use tauri::AppHandle;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ── Constants ──────────────────────────────────────────────────────────

fn client_id() -> &'static str {
    match option_env!("GOOGLE_CLIENT_ID") {
        Some(s) => s,
        None => "",
    }
}

fn client_secret() -> &'static str {
    match option_env!("GOOGLE_CLIENT_SECRET") {
        Some(s) => s,
        None => "",
    }
}

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Scope for Gemini / Gemma API access via Google Cloud.
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

// ── PKCE ───────────────────────────────────────────────────────────────

fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

// ── Localhost callback server ──────────────────────────────────────────

fn bind_loopback() -> Result<(u16, TcpListener), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Cannot bind loopback: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    Ok((port, listener))
}

fn build_auth_url(challenge: &str, redirect_uri: &str, state_param: &str) -> String {
    format!(
        "{AUTH_URL}?response_type=code\
         &client_id={cid}\
         &redirect_uri={redirect_uri}\
         &scope={SCOPE}\
         &code_challenge={challenge}\
         &code_challenge_method=S256\
         &state={state_param}\
         &access_type=offline\
         &prompt=consent",
        cid = client_id()
    )
}

async fn accept_callback(listener: TcpListener) -> Result<String, String> {
    let listener = tokio::net::TcpListener::from_std(listener)
        .map_err(|e| format!("async listener: {e}"))?;

    let (mut stream, _) = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        listener.accept(),
    )
    .await
    .map_err(|_| "OAuth timed out (5 min) waiting for browser sign-in".to_string())?
    .map_err(|e| format!("Accept error: {e}"))?;

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| format!("Read error: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse "GET /?code=xxx&state=yyy HTTP/1.1"
    let code = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| {
            url::form_urlencoded::parse(path.trim_start_matches("/?").as_bytes())
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.into_owned())
        })
        .ok_or_else(|| "No authorization code in callback URL".to_string())?;

    let html = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><head><script>window.close()</script></head>\
        <body style='font-family:sans-serif;padding:2rem;color:#1a1a1a'>\
        <h2 style='color:#4285F4'>Connected to Chirp</h2>\
        <p>You can close this tab and return to the app.</p>\
        </body></html>";
    let _ = stream.write_all(html).await;

    Ok(code)
}

// ── Token exchange ─────────────────────────────────────────────────────

async fn exchange_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id()),
        ("client_secret", client_secret()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Token exchange error {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse token response: {e}"))?;

    body["access_token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("No access_token in response: {body}"))
}

// ── Programmatic API key creation ─────────────────────────────────────
//
// The generativelanguage.googleapis.com endpoint only accepts API keys,
// not user OAuth tokens. So after sign-in we use the OAuth token to
// programmatically create a Gemini API key in the user's GCP project,
// store that key, and discard the OAuth token. The user never sees a key.

async fn get_first_project(access_token: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get("https://cloudresourcemanager.googleapis.com/v1/projects?filter=lifecycleState:ACTIVE")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("List projects failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("List projects error {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await
        .map_err(|e| format!("Parse projects: {e}"))?;

    body["projects"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|p| p["projectId"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No active GCP projects found on this account".to_string())
}

async fn create_gemini_api_key(access_token: &str, project_id: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    // Create the key
    let url = format!(
        "https://apikeys.googleapis.com/v2/projects/{project_id}/locations/global/keys"
    );
    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({ "displayName": "Chirp voice-to-text" }))
        .send()
        .await
        .map_err(|e| format!("Create API key failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Create API key error {status}: {body}"));
    }

    // Response is a long-running operation — poll until done
    let op: serde_json::Value = resp.json().await
        .map_err(|e| format!("Parse create key response: {e}"))?;

    let op_name = op["name"]
        .as_str()
        .ok_or_else(|| "No operation name in create key response".to_string())?;

    // Poll up to 10 times with 1s delay
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let poll_resp = client
            .get(format!("https://apikeys.googleapis.com/v2/{op_name}"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| format!("Poll operation failed: {e}"))?;

        let poll: serde_json::Value = poll_resp.json().await
            .map_err(|e| format!("Parse poll response: {e}"))?;

        if poll["done"].as_bool().unwrap_or(false) {
            return poll["response"]["keyString"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| format!("No keyString in operation result: {poll}"));
        }
    }

    Err("API key creation timed out".to_string())
}

// ── Public entry point ─────────────────────────────────────────────────

/// Returns true if this build was compiled with OAuth credentials.
pub fn is_configured() -> bool {
    !client_id().is_empty()
}

/// Start the Google OAuth PKCE flow. Returns the auth URL to open in the browser.
/// After sign-in, automatically creates a Gemini API key in the user's GCP project
/// and emits `google-auth-complete` with `{ "token": "AIza..." }` (the API key).
pub async fn start_flow(app: AppHandle) -> Result<String, String> {
    if client_id().is_empty() {
        return Err(
            "No Google OAuth client ID configured. \
             Register the app at console.cloud.google.com and rebuild with \
             GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET set."
                .to_string(),
        );
    }

    let verifier = generate_code_verifier();
    let challenge = code_challenge(&verifier);
    let state_param = generate_code_verifier();

    let (port, listener) = bind_loopback()?;
    let redirect_uri = format!("http://127.0.0.1:{port}/");
    let auth_url = build_auth_url(&challenge, &redirect_uri, &state_param);

    let app_clone = app.clone();
    tokio::spawn(async move {
        let result = async {
            let code = accept_callback(listener).await?;
            let oauth_token = exchange_code(&code, &verifier, &redirect_uri).await?;
            // Dynamically resolve the user's own GCP project so the API key
            // is created in their account (not the developer's project).
            let project_id = get_first_project(&oauth_token).await?;
            let api_key = create_gemini_api_key(&oauth_token, &project_id).await?;
            Ok::<String, String>(api_key)
        }
        .await;

        match result {
            Ok(token) => {
                let _ = app_clone.emit(
                    "google-auth-complete",
                    serde_json::json!({ "token": token }),
                );
            }
            Err(e) => {
                log::error!("Google OAuth flow failed: {e}");
                let _ = app_clone.emit(
                    "google-auth-error",
                    serde_json::json!({ "error": e }),
                );
            }
        }
    });

    Ok(auth_url)
}
