use super::*;
use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::header;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REVOKE_TOKEN_URL: &str = "https://auth.openai.com/oauth/revoke";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const ORIGINATOR: &str = "codex_cli_rs";
const AUTH_TYPE: &str = "chatgpt-codex";
const DEFAULT_EXPIRES_IN_SECS: u64 = 3600;
const REFRESH_SKEW_MS: u64 = 300_000;
const DEVICE_TIMEOUT_SECS: u64 = 15 * 60;
const OAUTH_SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const DEFAULT_SERVER_PORT: u16 = 1455;
const FALLBACK_SERVER_PORT: u16 = 1457;
const REVOKE_TIMEOUT_SECS: u64 = 10;
const CODEX_API_KEY_ENV: &str = "CODEX_API_KEY";
const CODEX_ACCESS_TOKEN_ENV: &str = "CODEX_ACCESS_TOKEN";
const AUTH_TYPE_API_KEY: &str = "api-key";
const AUTH_TYPE_PAT: &str = "personal-access-token";
const OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCodexAuth {
    #[serde(rename = "type")]
    auth_type: String,
    access: String,
    #[serde(default)]
    refresh: String,
    #[serde(default)]
    expires_at_ms: u64,
    #[serde(default)]
    account_id: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_in: Option<u64>,
}

#[derive(Debug)]
struct DeviceCode {
    device_auth_id: String,
    user_code: String,
    interval_secs: u64,
}

#[derive(Debug)]
struct AuthorizationCode {
    code: String,
    verifier: String,
}

#[derive(Debug)]
struct CodexRequestError {
    status: Option<StatusCode>,
    message: String,
}

impl fmt::Display for CodexRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CodexRequestError {}

struct PkceCodes {
    code_verifier: String,
    code_challenge: String,
}

fn generate_pkce() -> PkceCodes {
    use rand::RngExt;
    let mut bytes = [0u8; 64];
    rand::rng().fill(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

fn generate_state() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn build_authorize_url(redirect_uri: &str, pkce: &PkceCodes, state: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("scope", OAUTH_SCOPE),
        ("code_challenge", &pkce.code_challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", ORIGINATOR),
    ];
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{AUTHORIZE_URL}?{qs}")
}

fn bind_server() -> Result<tiny_http::Server> {
    tiny_http::Server::http(format!("127.0.0.1:{DEFAULT_SERVER_PORT}"))
        .or_else(|_| tiny_http::Server::http(format!("127.0.0.1:{FALLBACK_SERVER_PORT}")))
        .map_err(|e| anyhow!("failed to start local auth server: {e}"))
}

fn wait_for_callback(server: &Arc<tiny_http::Server>, expected_state: &str) -> Result<String> {
    loop {
        let request = server.recv().map_err(|e| anyhow!("server recv error: {e}"))?;
        let url_str = format!("http://localhost{}", request.url());
        let parsed = url::Url::parse(&url_str).context("failed to parse callback URL")?;

        if !parsed.path().starts_with("/auth/callback") {
            let response = tiny_http::Response::from_string("Not found")
                .with_status_code(tiny_http::StatusCode(404));
            let _ = request.respond(response);
            continue;
        }

        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();

        if let Some(error) = params.get("error") {
            let desc = params
                .get("error_description")
                .map(|s| s.to_string())
                .unwrap_or_default();
            let html = format!(
                "<html><body><h2>Authentication failed</h2><p>{error}: {desc}</p></body></html>"
            );
            let response = tiny_http::Response::from_string(html)
                .with_header("Content-Type: text/html".parse::<tiny_http::Header>().unwrap())
                .with_status_code(tiny_http::StatusCode(400));
            let _ = request.respond(response);
            return Err(anyhow!("OAuth error: {error} - {desc}"));
        }

        let state = params
            .get("state")
            .ok_or_else(|| anyhow!("callback missing state parameter"))?;
        if state.as_ref() != expected_state {
            let response = tiny_http::Response::from_string("Invalid state")
                .with_status_code(tiny_http::StatusCode(400));
            let _ = request.respond(response);
            continue;
        }

        let code = params
            .get("code")
            .ok_or_else(|| anyhow!("callback missing code parameter"))?
            .to_string();

        let html = "<html><body><h2>Authentication successful</h2><p>You can close this tab and return to the terminal.</p></body></html>";
        let response = tiny_http::Response::from_string(html)
            .with_header("Content-Type: text/html".parse::<tiny_http::Header>().unwrap());
        let _ = request.respond(response);

        return Ok(code);
    }
}

async fn exchange_code_pkce(
    client: &Client,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> Result<TokenResponse> {
    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", pkce.code_verifier.as_str()),
        ])
        .send()
        .await
        .context("failed to exchange authorization code")?;

    parse_token_response(response, "browser token exchange").await
}

async fn browser_login() -> Result<()> {
    let pkce = generate_pkce();
    let state = generate_state();
    let server = Arc::new(bind_server()?);
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("unable to determine server port"))?;
    let redirect_uri = format!("http://localhost:{}/auth/callback", addr.port());
    let auth_url = build_authorize_url(&redirect_uri, &pkce, &state);

    if let Err(e) = webbrowser::open(&auth_url) {
        eprintln!("Failed to open browser: {e}");
    }
    eprintln!("If your browser did not open, navigate to:");
    eprintln!("{auth_url}");
    eprintln!();
    eprintln!("Waiting for authentication...");

    let server_clone = Arc::clone(&server);
    let state_clone = state.clone();
    let code =
        tokio::task::spawn_blocking(move || wait_for_callback(&server_clone, &state_clone))
            .await
            .context("callback handler panicked")??;

    let client = Client::new();
    let tokens = exchange_code_pkce(&client, &redirect_uri, &pkce, &code).await?;
    let auth = auth_from_token_response(tokens, None)?;
    with_auth_lock(|| write_auth_file(&auth))?;

    println!("Codex auth saved.");
    println!("account: {}", auth.account_id);
    println!("path: {}", auth_file_path()?.display());
    Ok(())
}

async fn device_code_login() -> Result<()> {
    let client = Client::new();
    let device = request_device_code(&client).await?;

    println!("Open this URL in a browser:");
    println!("{DEVICE_VERIFICATION_URL}");
    println!();
    println!("Enter this code:");
    println!("{}", device.user_code);
    println!();
    println!("Waiting for authorization...");

    let code = poll_device_code(&client, &device).await?;
    let tokens = exchange_authorization_code(&client, &code).await?;
    let auth = auth_from_token_response(tokens, None)?;
    with_auth_lock(|| write_auth_file(&auth))?;

    println!("Codex auth saved.");
    println!("account: {}", auth.account_id);
    println!("path: {}", auth_file_path()?.display());
    Ok(())
}

pub async fn codex_auth_login(headless: bool) -> Result<()> {
    if headless {
        return device_code_login().await;
    }
    match browser_login().await {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("Browser login failed: {e}");
            eprintln!("Falling back to device code flow...");
            device_code_login().await
        }
    }
}

pub fn codex_auth_login_api_key(api_key: &str) -> Result<()> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(anyhow!("API key is empty"));
    }
    let auth = StoredCodexAuth {
        auth_type: AUTH_TYPE_API_KEY.to_string(),
        access: key.to_string(),
        refresh: String::new(),
        expires_at_ms: 0,
        account_id: String::new(),
    };
    with_auth_lock(|| write_auth_file(&auth))?;
    println!("API key auth saved.");
    println!("path: {}", auth_file_path()?.display());
    Ok(())
}

pub fn codex_auth_login_access_token(token: &str) -> Result<()> {
    let token = token.trim();
    if token.is_empty() {
        return Err(anyhow!("access token is empty"));
    }
    let auth = StoredCodexAuth {
        auth_type: AUTH_TYPE_PAT.to_string(),
        access: token.to_string(),
        refresh: String::new(),
        expires_at_ms: 0,
        account_id: String::new(),
    };
    with_auth_lock(|| write_auth_file(&auth))?;
    println!("Access token auth saved.");
    println!("path: {}", auth_file_path()?.display());
    Ok(())
}

async fn revoke_token(client: &Client, token: &str, token_type_hint: &str) -> Result<()> {
    let mut body = json!({
        "token": token,
        "token_type_hint": token_type_hint,
    });
    if token_type_hint == "refresh_token" {
        body["client_id"] = Value::String(CLIENT_ID.to_string());
    }

    let result = client
        .post(REVOKE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", codex_user_agent())
        .timeout(std::time::Duration::from_secs(REVOKE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!("token revoked successfully");
            Ok(())
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!("token revocation returned HTTP {}: {}", status, truncate(&text));
            Ok(())
        }
        Err(e) => {
            tracing::warn!("token revocation request failed: {e}");
            Ok(())
        }
    }
}

pub async fn codex_auth_logout() -> Result<()> {
    let path = auth_file_path()?;

    if let Ok(Some(auth)) = with_auth_lock(|| read_auth_file_optional()) {
        if auth.auth_type == AUTH_TYPE && !auth.refresh.is_empty() {
            let client = Client::new();
            let _ = revoke_token(&client, &auth.refresh, "refresh_token").await;
        }
    }

    let removed = with_auth_lock(|| match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    })?;

    if removed {
        println!("Codex auth removed.");
    } else {
        println!("No Codex auth found.");
    }
    println!("path: {}", path.display());
    Ok(())
}

pub fn codex_auth_status() -> Result<()> {
    let path = auth_file_path()?;

    if let Ok(key) = std::env::var(CODEX_API_KEY_ENV) {
        if !key.is_empty() {
            println!("Codex auth: {} (via {CODEX_API_KEY_ENV} env)", AUTH_TYPE_API_KEY);
            return Ok(());
        }
    }
    if let Ok(token) = std::env::var(CODEX_ACCESS_TOKEN_ENV) {
        if !token.is_empty() {
            let mode = if token.starts_with("at-") { AUTH_TYPE_PAT } else { "access-token" };
            println!("Codex auth: {mode} (via {CODEX_ACCESS_TOKEN_ENV} env)");
            return Ok(());
        }
    }

    let auth = with_auth_lock(|| read_auth_file_optional())?;
    match auth {
        Some(auth) => {
            println!("Codex auth: signed in ({})", auth.auth_type);
            if !auth.account_id.is_empty() {
                println!("account: {}", auth.account_id);
            }
            if auth.expires_at_ms > 0 {
                println!("expires: {}", expiry_status(auth.expires_at_ms));
            }
            println!("path: {}", path.display());
        }
        None => {
            println!("Codex auth: not signed in");
            println!("path: {}", path.display());
        }
    }
    Ok(())
}

pub async fn send_responses(
    client: &Client,
    base_url: &str,
    model: &str,
    reasoning_effort: Option<&ReasoningEffort>,
    reasoning_summary: Option<&ReasoningSummary>,
    reasoning_context: Option<&ReasoningContext>,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
) -> Result<ModelTurnResponse> {
    let auth = fresh_auth(client).await?;
    let url = if auth.auth_type == AUTH_TYPE_API_KEY {
        format!("{}/responses", OPENAI_API_BASE_URL.trim_end_matches('/'))
    } else {
        codex_responses_url(base_url)
    };
    let request = codex_responses_request(model, reasoning_effort, reasoning_summary, reasoning_context, &messages, &tools);
    let started = Instant::now();
    tracing::info!(
        backend = ?BackendKind::ChatGptCodexResponses,
        model = %model,
        auth_type = %auth.auth_type,
        reasoning_effort = ?reasoning_effort,
        message_count = messages.len(),
        tool_count = tools.len(),
        request_bytes = serde_json::to_vec(&request)?.len(),
        "starting codex responses turn"
    );

    match post_codex_json_with_retry(client, &url, &request, &auth).await {
        Ok(value) => {
            let parsed = parse_openai_responses_response(&value, &url)?;
            tracing::info!(
                finish_reason = ?parsed.finish_reason,
                has_text = parsed.assistant.content.is_some(),
                tool_call_count = parsed
                    .assistant
                    .tool_calls
                    .as_ref()
                    .map(|calls| calls.len())
                    .unwrap_or(0),
                latency_ms = started.elapsed().as_millis() as u64,
                "codex responses turn completed"
            );
            Ok(parsed)
        }
        Err(error) if error.status == Some(StatusCode::UNAUTHORIZED) => {
            tracing::warn!("codex responses request returned 401; forcing auth refresh");
            let refreshed = force_refresh_auth(client).await?;
            let value = post_codex_json_with_retry(client, &url, &request, &refreshed)
                .await
                .map_err(anyhow::Error::new)?;
            let parsed = parse_openai_responses_response(&value, &url)?;
            tracing::info!(
                finish_reason = ?parsed.finish_reason,
                has_text = parsed.assistant.content.is_some(),
                tool_call_count = parsed
                    .assistant
                    .tool_calls
                    .as_ref()
                    .map(|calls| calls.len())
                    .unwrap_or(0),
                latency_ms = started.elapsed().as_millis() as u64,
                refreshed_auth = true,
                "codex responses turn completed after auth refresh"
            );
            Ok(parsed)
        }
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

async fn request_device_code(client: &Client) -> Result<DeviceCode> {
    let response = client
        .post(DEVICE_USER_CODE_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", codex_user_agent())
        .json(&json!({ "client_id": CLIENT_ID }))
        .send()
        .await
        .context("failed to request Codex device code")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Codex device-code response")?;
    if !status.is_success() {
        return Err(anyhow!(
            "Codex device-code request failed with HTTP {}: {}",
            status.as_u16(),
            truncate(&body)
        ));
    }

    let value: Value =
        serde_json::from_str(&body).context("failed to parse Codex device-code response")?;
    let device_auth_id = value
        .get("device_auth_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Codex device-code response did not include device_auth_id"))?
        .to_string();
    let user_code = value
        .get("user_code")
        .or_else(|| value.get("usercode"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Codex device-code response did not include user_code"))?
        .to_string();
    let interval_secs = interval_secs(value.get("interval")).unwrap_or(5).max(1);

    Ok(DeviceCode {
        device_auth_id,
        user_code,
        interval_secs,
    })
}

async fn poll_device_code(client: &Client, device: &DeviceCode) -> Result<AuthorizationCode> {
    let started = now_ms();
    loop {
        let response = client
            .post(DEVICE_TOKEN_URL)
            .header("Content-Type", "application/json")
            .header("User-Agent", codex_user_agent())
            .json(&json!({
                "device_auth_id": device.device_auth_id,
                "user_code": device.user_code,
            }))
            .send()
            .await
            .context("failed to poll Codex device authorization")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read Codex device authorization response")?;

        if status.is_success() {
            let value: Value = serde_json::from_str(&body)
                .context("failed to parse Codex device authorization response")?;
            let code = value
                .get("authorization_code")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "Codex device authorization response did not include authorization_code"
                    )
                })?
                .to_string();
            let verifier = value
                .get("code_verifier")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!("Codex device authorization response did not include code_verifier")
                })?
                .to_string();
            return Ok(AuthorizationCode { code, verifier });
        }

        if status != StatusCode::FORBIDDEN && status != StatusCode::NOT_FOUND {
            return Err(anyhow!(
                "Codex device authorization failed with HTTP {}: {}",
                status.as_u16(),
                truncate(&body)
            ));
        }

        if now_ms().saturating_sub(started) >= DEVICE_TIMEOUT_SECS * 1000 {
            return Err(anyhow!(
                "Codex device authorization timed out after 15 minutes"
            ));
        }

        sleep(Duration::from_secs(device.interval_secs)).await;
    }
}

async fn exchange_authorization_code(
    client: &Client,
    code: &AuthorizationCode,
) -> Result<TokenResponse> {
    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.code.as_str()),
            ("redirect_uri", DEVICE_REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", code.verifier.as_str()),
        ])
        .send()
        .await
        .context("failed to exchange Codex authorization code")?;

    parse_token_response(response, "Codex token exchange").await
}

async fn refresh_access_token(client: &Client, refresh_token: &str) -> Result<TokenResponse> {
    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", codex_user_agent())
        .json(&json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .context("failed to refresh Codex access token")?;

    parse_token_response(response, "Codex token refresh").await
}

async fn parse_token_response(response: reqwest::Response, label: &str) -> Result<TokenResponse> {
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read {label} response"))?;
    if !status.is_success() {
        return Err(anyhow!(
            "{label} failed with HTTP {}: {}",
            status.as_u16(),
            truncate(&body)
        ));
    }
    serde_json::from_str(&body).with_context(|| format!("failed to parse {label} response"))
}

fn resolve_auth_from_env() -> Option<StoredCodexAuth> {
    if let Ok(key) = std::env::var(CODEX_API_KEY_ENV) {
        if !key.is_empty() {
            return Some(StoredCodexAuth {
                auth_type: AUTH_TYPE_API_KEY.to_string(),
                access: key,
                refresh: String::new(),
                expires_at_ms: 0,
                account_id: String::new(),
            });
        }
    }
    if let Ok(token) = std::env::var(CODEX_ACCESS_TOKEN_ENV) {
        if !token.is_empty() {
            let auth_type = if token.starts_with("at-") {
                AUTH_TYPE_PAT
            } else {
                AUTH_TYPE
            };
            return Some(StoredCodexAuth {
                auth_type: auth_type.to_string(),
                access: token,
                refresh: String::new(),
                expires_at_ms: 0,
                account_id: String::new(),
            });
        }
    }
    None
}

fn auth_needs_refresh(auth: &StoredCodexAuth) -> bool {
    auth.auth_type == AUTH_TYPE
        && auth.expires_at_ms > 0
        && !auth.refresh.is_empty()
}

async fn fresh_auth(client: &Client) -> Result<StoredCodexAuth> {
    if let Some(auth) = resolve_auth_from_env() {
        return Ok(auth);
    }
    let _lock = acquire_auth_lock()?;
    let auth = read_auth_file()?;
    if !auth_needs_refresh(&auth) || auth.expires_at_ms > now_ms().saturating_add(REFRESH_SKEW_MS) {
        tracing::debug!(
            remaining_ms = auth.expires_at_ms.saturating_sub(now_ms()),
            "reusing existing codex auth token"
        );
        return Ok(auth);
    }
    tracing::info!(
        remaining_ms = auth.expires_at_ms.saturating_sub(now_ms()),
        refresh_trigger = "expiry",
        "refreshing codex auth because token is near expiry"
    );
    refresh_and_store_auth(client, auth).await
}

async fn force_refresh_auth(client: &Client) -> Result<StoredCodexAuth> {
    if let Some(auth) = resolve_auth_from_env() {
        return Err(anyhow!(
            "401 Unauthorized with {} auth (env var). Check your key/token.",
            auth.auth_type
        ));
    }
    let _lock = acquire_auth_lock()?;
    let auth = read_auth_file()?;
    if !auth_needs_refresh(&auth) {
        return Err(anyhow!(
            "401 Unauthorized with {} auth. Re-run `sac codex-auth login` to re-authenticate.",
            auth.auth_type
        ));
    }
    tracing::warn!(
        refresh_trigger = "401",
        "forcing codex auth refresh after unauthorized response"
    );
    refresh_and_store_auth(client, auth).await
}

async fn refresh_and_store_auth(
    client: &Client,
    current: StoredCodexAuth,
) -> Result<StoredCodexAuth> {
    let started = Instant::now();
    let tokens = refresh_access_token(client, &current.refresh).await?;
    let refreshed = auth_from_token_response(tokens, Some(&current.account_id))?;
    write_auth_file(&refreshed)?;
    tracing::info!(
        latency_ms = started.elapsed().as_millis() as u64,
        "codex auth refresh persisted"
    );
    Ok(refreshed)
}

fn auth_from_token_response(
    response: TokenResponse,
    fallback_account_id: Option<&str>,
) -> Result<StoredCodexAuth> {
    let account_id = response
        .id_token
        .as_deref()
        .and_then(extract_account_id)
        .or_else(|| extract_account_id(&response.access_token))
        .or(fallback_account_id.map(str::to_string))
        .ok_or_else(|| anyhow!("Codex token response did not include a ChatGPT account id"))?;
    let expires_in = response.expires_in.unwrap_or(DEFAULT_EXPIRES_IN_SECS);
    Ok(StoredCodexAuth {
        auth_type: AUTH_TYPE.to_string(),
        access: response.access_token,
        refresh: response.refresh_token,
        expires_at_ms: now_ms().saturating_add(expires_in.saturating_mul(1000)),
        account_id,
    })
}

fn codex_responses_request(
    model: &str,
    reasoning_effort: Option<&ReasoningEffort>,
    reasoning_summary: Option<&ReasoningSummary>,
    reasoning_context: Option<&ReasoningContext>,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> Value {
    let (instructions, input) = codex_instructions_and_input(messages);
    let mut request = json!({
        "model": model,
        "input": input,
        "store": false,
        "stream": true,
        "text": {
            "verbosity": "low",
        },
        "tool_choice": "auto",
        "parallel_tool_calls": true,
    });

    if let Some(instructions) = instructions {
        request["instructions"] = Value::String(instructions);
    }

    if !tools.is_empty() {
        request["tools"] = Value::Array(
            tools
                .iter()
                .map(openai_responses_tool_to_value)
                .collect::<Vec<_>>(),
        );
    }

    if let Some(effort) = reasoning_effort {
        let mut reasoning = json!({
            "effort": effort.as_str(),
        });
        if let Some(summary) = reasoning_summary {
            reasoning["summary"] = json!(summary.as_str());
        }
        if let Some(context) = reasoning_context {
            reasoning["context"] = json!(context.as_str());
        }
        request["reasoning"] = reasoning;
        request["include"] = json!(["reasoning.encrypted_content"]);
    }

    request
}

fn codex_instructions_and_input(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut instructions = Vec::new();
    let mut input_messages = Vec::new();

    for message in messages {
        match message {
            Message::System { content } => {
                if !content.trim().is_empty() {
                    instructions.push(content.clone());
                }
            }
            _ => input_messages.push(message.clone()),
        }
    }

    let instructions = if instructions.is_empty() {
        None
    } else {
        Some(instructions.join("\n\n"))
    };
    (instructions, responses_input_items(&input_messages))
}

async fn post_codex_json_with_retry(
    client: &Client,
    url: &str,
    body: &Value,
    auth: &StoredCodexAuth,
) -> std::result::Result<Value, CodexRequestError> {
    let mut last_error = CodexRequestError {
        status: None,
        message: "No attempts made".to_string(),
    };
    let request_bytes = serde_json::to_vec(body)
        .map(|bytes| bytes.len())
        .unwrap_or(0);

    for attempt in 0..3 {
        if attempt > 0 {
            let delay_secs = 1u64 << (attempt - 1);
            tracing::warn!(
                attempt = attempt + 1,
                backoff_secs = delay_secs,
                endpoint = "responses",
                "retrying codex HTTP request after backoff"
            );
            sleep(Duration::from_secs(delay_secs)).await;
        }

        let attempt_started = Instant::now();
        tracing::debug!(
            attempt = attempt + 1,
            endpoint = "responses",
            request_bytes,
            "starting codex HTTP attempt"
        );

        let mut req = client
            .post(url)
            .header("Authorization", format!("Bearer {}", auth.access))
            .header("originator", ORIGINATOR)
            .header("User-Agent", codex_user_agent())
            .header(header::ACCEPT, "text/event-stream")
            .header(header::CONTENT_TYPE, "application/json");

        if !auth.account_id.is_empty() {
            req = req.header("ChatGPT-Account-Id", auth.account_id.as_str());
        }
        if auth.auth_type == AUTH_TYPE_API_KEY {
            req = req.header("OpenAI-Beta", "responses=v1");
        }

        let response = req
            .json(body)
            .send()
            .await
            .map_err(|error| CodexRequestError {
                status: None,
                message: format!("HTTP request failed for {url}: {error}"),
            })?;

        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let response_body = response.text().await.map_err(|error| CodexRequestError {
            status: Some(status),
            message: format!("Failed to read response body from {url}: {error}"),
        })?;
        let response_bytes = response_body.len();

        if status.is_success() {
            tracing::info!(attempt = attempt + 1, status = status.as_u16(), endpoint = "responses", request_bytes, response_bytes, latency_ms = attempt_started.elapsed().as_millis() as u64, content_type = ?content_type, "codex HTTP attempt succeeded");
            return parse_codex_success_body(url, status, content_type.as_deref(), &response_body);
        }

        let error = CodexRequestError {
            status: Some(status),
            message: format!(
                "HTTP {} from {url}: {}",
                status.as_u16(),
                truncate(&response_body)
            ),
        };
        if status == StatusCode::UNAUTHORIZED {
            tracing::warn!(
                attempt = attempt + 1,
                status = status.as_u16(),
                request_bytes,
                response_bytes,
                latency_ms = attempt_started.elapsed().as_millis() as u64,
                endpoint = "responses",
                retryable = false,
                "codex HTTP attempt failed with unauthorized status"
            );
            return Err(error);
        }
        if status.as_u16() == 429 || status.is_server_error() {
            tracing::warn!(
                attempt = attempt + 1,
                status = status.as_u16(),
                request_bytes,
                response_bytes,
                latency_ms = attempt_started.elapsed().as_millis() as u64,
                endpoint = "responses",
                retryable = true,
                "codex HTTP attempt failed with retryable status"
            );
            last_error = error;
            continue;
        }
        tracing::error!(
            attempt = attempt + 1,
            status = status.as_u16(),
            request_bytes,
            response_bytes,
            latency_ms = attempt_started.elapsed().as_millis() as u64,
            endpoint = "responses",
            retryable = false,
            "codex HTTP attempt failed with non-retryable status"
        );
        return Err(error);
    }

    Err(last_error)
}

fn parse_codex_success_body(
    url: &str,
    status: StatusCode,
    content_type: Option<&str>,
    response_body: &str,
) -> std::result::Result<Value, CodexRequestError> {
    let looks_like_sse = content_type
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false)
        || response_body.lines().any(|line| line.starts_with("data:"));
    tracing::debug!(status = status.as_u16(), response_bytes = response_body.len(), looks_like_sse, content_type = ?content_type, endpoint = "responses", "parsing codex success body");
    if looks_like_sse {
        return parse_codex_sse_response(response_body).map_err(|message| CodexRequestError {
            status: Some(status),
            message: format!(
                "Failed to parse SSE response from {url}: {message}\nBody: {}",
                truncate(response_body)
            ),
        });
    }

    serde_json::from_str::<Value>(response_body).map_err(|error| CodexRequestError {
        status: Some(status),
        message: format!(
            "Failed to parse response from {url}: {error}\nBody: {}",
            truncate(response_body)
        ),
    })
}

fn parse_codex_sse_response(response_body: &str) -> std::result::Result<Value, String> {
    let mut final_response = None;
    let mut output_items: Vec<(usize, Value)> = Vec::new();
    let started = Instant::now();
    let payloads = sse_data_payloads(response_body);
    let payload_count = payloads.len();
    let mut terminal_event: Option<String> = None;

    for data in payloads {
        if data == "[DONE]" {
            continue;
        }

        let event: Value = serde_json::from_str(&data)
            .map_err(|error| format!("invalid SSE JSON event: {error}"))?;
        match event.get("type").and_then(Value::as_str) {
            Some("error") | Some("response.failed") => {
                return Err(codex_event_error_message(&event)
                    .unwrap_or_else(|| format!("Codex error event: {event}")));
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item").cloned() {
                    let output_index = event
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .and_then(|index| usize::try_from(index).ok())
                        .unwrap_or(output_items.len());
                    output_items.retain(|(index, _)| *index != output_index);
                    output_items.push((output_index, item));
                }
            }
            Some("response.completed") | Some("response.done") | Some("response.incomplete") => {
                terminal_event = event
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(response) = event.get("response").and_then(Value::as_object) {
                    if response.get("status").and_then(Value::as_str) == Some("failed") {
                        return Err(codex_event_error_message(&event)
                            .unwrap_or_else(|| format!("Codex response failed: {event}")));
                    }
                    let mut response_value = Value::Object(response.clone());
                    if response_output_is_empty(&response_value) && !output_items.is_empty() {
                        output_items.sort_by_key(|(index, _)| *index);
                        response_value["output"] = Value::Array(
                            output_items
                                .iter()
                                .map(|(_, item)| item.clone())
                                .collect::<Vec<_>>(),
                        );
                    }
                    final_response = Some(response_value);
                }
            }
            _ => {}
        }
    }

    tracing::info!(
        payload_count,
        output_item_done_count = output_items.len(),
        terminal_event = ?terminal_event,
        response_bytes = response_body.len(),
        latency_ms = started.elapsed().as_millis() as u64,
        "parsed codex SSE response"
    );

    final_response.ok_or_else(|| "SSE stream did not include a final response event".to_string())
}

fn response_output_is_empty(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .map(Vec::is_empty)
        .unwrap_or(true)
}

fn sse_data_payloads(response_body: &str) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut current = String::new();

    for line in response_body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(data.trim_start());
        } else if line.trim().is_empty() && !current.is_empty() {
            payloads.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        payloads.push(current);
    }

    payloads
}

fn codex_event_error_message(event: &Value) -> Option<String> {
    event
        .get("response")
        .and_then(|response| response.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
        .or_else(|| event.get("message").and_then(Value::as_str))
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

fn codex_responses_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url.trim()
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_string()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

fn auth_file_path() -> Result<PathBuf> {
    crate::paths::sac_home_dir()
        .map(|dir| dir.join("auth.json"))
        .ok_or_else(|| anyhow!("could not determine SAC_HOME or HOME for Codex auth storage"))
}

fn auth_lock_path() -> Result<PathBuf> {
    Ok(auth_file_path()?.with_file_name("auth.json.lock"))
}

fn acquire_auth_lock() -> Result<FileLock> {
    let path = auth_lock_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    FileLock::acquire(&path)
}

fn with_auth_lock<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock = acquire_auth_lock()?;
    let result = operation();
    drop(lock);
    result
}

fn read_auth_file_optional() -> Result<Option<StoredCodexAuth>> {
    let path = auth_file_path()?;
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    };
    let auth: StoredCodexAuth = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let valid_types = [AUTH_TYPE, AUTH_TYPE_API_KEY, AUTH_TYPE_PAT];
    if !valid_types.contains(&auth.auth_type.as_str()) {
        return Err(anyhow!(
            "{} contains unsupported auth type '{}'",
            path.display(),
            auth.auth_type
        ));
    }
    Ok(Some(auth))
}

fn read_auth_file() -> Result<StoredCodexAuth> {
    read_auth_file_optional()?.ok_or_else(|| {
        anyhow!("Codex auth is not configured. Run `sac codex-auth` to sign in with ChatGPT.")
    })
}

fn write_auth_file(auth: &StoredCodexAuth) -> Result<()> {
    let path = auth_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(auth).context("failed to serialize Codex auth")?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    use std::io::Write;
    file.write_all(raw.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("failed to open auth lock {}", path.display()))?;
        lock_file(&file).with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn lock_file(file: &File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn lock_file(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn unlock_file(_file: &File) -> io::Result<()> {
    Ok(())
}

fn extract_account_id(token: &str) -> Option<String> {
    let payload = decode_jwt_payload(token)?;
    payload
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("chatgpt_account_id").and_then(Value::as_str))
        .or_else(|| {
            payload
                .get("organizations")
                .and_then(Value::as_array)
                .and_then(|orgs| orgs.first())
                .and_then(|org| org.get("id"))
                .and_then(Value::as_str)
        })
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    let bytes = base64_url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut out = Vec::with_capacity(input.len() * 3 / 4);

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        bits = (bits << 6) | value;
        bit_count += 6;
        while bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }

    Some(out)
}

fn interval_secs(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64(),
        Some(Value::String(text)) => text.parse().ok(),
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn expiry_status(expires_at_ms: u64) -> String {
    let now = now_ms();
    if expires_at_ms <= now {
        let seconds = now.saturating_sub(expires_at_ms) / 1000;
        format!("expired {seconds}s ago")
    } else {
        let seconds = expires_at_ms.saturating_sub(now) / 1000;
        format!("in {}s", seconds)
    }
}

fn codex_user_agent() -> String {
    format!("sac/{}", env!("CARGO_PKG_VERSION"))
}

fn truncate(value: &str) -> String {
    value.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_codex_responses_urls() {
        assert_eq!(
            codex_responses_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            codex_responses_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            codex_responses_url("https://chatgpt.com/backend-api/codex/responses"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn extracts_account_id_from_nested_jwt_claim() {
        let token = concat!(
            "e30.",
            "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOns",
            "iY2hhdGdwdF9hY2NvdW50X2lkIjoid29ya3NwYWNlLTEyMyJ9fQ.",
            "sig"
        );

        assert_eq!(extract_account_id(token).as_deref(), Some("workspace-123"));
    }

    #[test]
    fn builds_codex_responses_stream_request() {
        let request = codex_responses_request(
            "gpt-5.5",
            Some(&ReasoningEffort::High),
            None,
            None,
            &[
                Message::System {
                    content: "system instructions".to_string(),
                },
                Message::User {
                    content: "hello".to_string(),
                },
            ],
            &[],
        );

        assert_eq!(request["model"], "gpt-5.5");
        assert_eq!(request["instructions"], "system instructions");
        assert_eq!(request["store"], false);
        assert_eq!(request["stream"], true);
        assert_eq!(request["text"]["verbosity"], "low");
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(request["parallel_tool_calls"], true);
        assert_eq!(request["reasoning"]["effort"], "high");
        assert_eq!(request["include"][0], "reasoning.encrypted_content");
        assert_eq!(request["input"].as_array().unwrap().len(), 1);
        assert_eq!(request["input"][0]["role"], "user");
    }

    #[test]
    fn parses_codex_sse_final_response() {
        let body = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
            "data: [DONE]\n\n"
        );

        let parsed = parse_codex_sse_response(body).unwrap();
        assert_eq!(parsed["status"], "completed");
        assert_eq!(parsed["output"][0]["type"], "message");
        assert_eq!(parsed["output"][0]["content"][0]["text"], "hello");
        assert_eq!(parsed["usage"]["total_tokens"], 3);
    }
}
