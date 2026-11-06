//! Microsoft account sign-in.
//!
//! Uses the OAuth 2.0 device-code flow: no embedded webview and no localhost
//! redirect. The user opens `microsoft.com/link`, types a short code, and the
//! launcher polls in the background. Tokens are stored in the system keyring
//! (Secret Service on Linux) and refreshed silently on startup.
//!
//! Chain: Microsoft OAuth -> Xbox Live -> XSTS -> Minecraft services -> profile.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::http;
use crate::state::{AccountKind, AccountSummary};

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const SCOPE: &str = "XboxLive.signin offline_access";

const KEYRING_SERVICE: &str = "dev.kestrel.launcher";
const KEYRING_USER: &str = "microsoft";

/// Refresh a little before the server-side expiry to avoid races.
const EXPIRY_SKEW_SECS: u64 = 120;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: String,
    pub xuid: String,
    pub client_id: String,
    pub expires_at: u64,
}

impl AuthSession {
    pub fn uuid_undashed(&self) -> String {
        self.uuid.replace('-', "")
    }

    pub fn summary(&self) -> AccountSummary {
        AccountSummary {
            id: self.uuid.clone(),
            username: self.username.clone(),
            uuid: self.uuid.clone(),
            kind: AccountKind::Microsoft,
        }
    }

    pub fn dev_enabled() -> bool {
        matches!(
            std::env::var("KESTREL_DEV_AUTH").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        )
    }

    /// Placeholder identity for smoke-testing the launch pipeline against the
    /// title screen. Online servers reject this token by design.
    pub fn dev() -> Result<Self> {
        if !Self::dev_enabled() {
            return Err(Error::msg(
                "dev auth is disabled; set KESTREL_DEV_AUTH=1 to launch with a placeholder token",
            ));
        }
        let username = std::env::var("KESTREL_DEV_NAME").unwrap_or_else(|_| "Dev".to_string());
        Ok(Self {
            username,
            uuid: "00000000000000000000000000000001".to_string(),
            access_token: "0".to_string(),
            user_type: "msa".to_string(),
            xuid: String::new(),
            client_id: String::new(),
            expires_at: now_secs() + 86_400,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTokens {
    pub client_id: String,
    pub ms_refresh_token: String,
    pub ms_access_token: String,
    pub ms_expires_at: u64,
    pub mc_access_token: String,
    pub mc_expires_at: u64,
    pub username: String,
    pub uuid: String,
    pub xuid: String,
    pub user_hash: String,
}

impl StoredTokens {
    pub fn session(&self) -> AuthSession {
        AuthSession {
            username: self.username.clone(),
            uuid: self.uuid.clone(),
            access_token: self.mc_access_token.clone(),
            user_type: "msa".to_string(),
            xuid: self.xuid.clone(),
            client_id: self.client_id.clone(),
            expires_at: self.mc_expires_at,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeInfo {
    #[serde(skip_serializing)]
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct PendingLogin {
    pub info: DeviceCodeInfo,
    started: Instant,
}

impl PendingLogin {
    pub fn new(info: DeviceCodeInfo) -> Self {
        Self {
            info,
            started: Instant::now(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.started.elapsed() > Duration::from_secs(self.info.expires_in)
    }
}

#[derive(Clone, Debug)]
pub struct MsToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
}

#[derive(Clone, Debug)]
pub enum PollOutcome {
    Pending,
    SlowDown,
    Complete(MsToken),
    Expired,
    Declined,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawMsToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: u64,
}

pub async fn start_device_code(client_id: &str) -> Result<DeviceCodeInfo> {
    if client_id.trim().is_empty() {
        return Err(Error::Config(
            "Microsoft sign-in is not configured: set azureClientId in config.json \
             (a public client with the device-code flow enabled)"
                .into(),
        ));
    }
    let response = http::client()
        .post(DEVICE_CODE_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(Error::msg(format!(
            "Microsoft device-code request failed (HTTP {status}): {}",
            describe_auth_error(&text)
        )));
    }
    let raw: DeviceCodeResponse =
        serde_json::from_str(&text).map_err(|e| Error::json("device code response", e))?;
    Ok(DeviceCodeInfo {
        device_code: raw.device_code,
        user_code: raw.user_code,
        verification_uri: raw.verification_uri,
        expires_in: raw.expires_in,
        interval: raw.interval.max(1),
        message: raw.message,
    })
}

pub async fn poll_device_code(client_id: &str, device_code: &str) -> Result<PollOutcome> {
    let response = http::client()
        .post(TOKEN_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", client_id),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if status.is_success() {
        let raw: RawMsToken =
            serde_json::from_str(&text).map_err(|e| Error::json("token response", e))?;
        return Ok(PollOutcome::Complete(MsToken {
            access_token: raw.access_token,
            refresh_token: raw.refresh_token,
            expires_in: raw.expires_in,
        }));
    }

    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let code = value["error"].as_str().unwrap_or_default();
    match code {
        "authorization_pending" => Ok(PollOutcome::Pending),
        "slow_down" => Ok(PollOutcome::SlowDown),
        "expired_token" | "bad_verification_code" => Ok(PollOutcome::Expired),
        "authorization_declined" => Ok(PollOutcome::Declined),
        _ => Err(Error::msg(format!(
            "Microsoft sign-in failed: {}",
            describe_auth_error(&text)
        ))),
    }
}

async fn refresh_ms_token(client_id: &str, refresh_token: &str) -> Result<RawMsToken> {
    let response = http::client()
        .post(TOKEN_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", SCOPE),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|e| Error::json("token response", e));
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if value["error"].as_str() == Some("invalid_grant") {
        let _ = delete_tokens();
        return Err(Error::AuthExpired);
    }
    Err(Error::msg(format!(
        "Microsoft token refresh failed (HTTP {status}): {}",
        describe_auth_error(&text)
    )))
}

/// Finish a login: Xbox Live -> XSTS -> Minecraft -> profile, persist and return.
pub async fn complete_login(
    client_id: &str,
    token: MsToken,
) -> Result<(AuthSession, StoredTokens)> {
    let chain = complete_chain(&token.access_token).await?;
    let refresh_token = token
        .refresh_token
        .ok_or_else(|| Error::msg("Microsoft did not return a refresh token"))?;
    let now = now_secs();
    let tokens = StoredTokens {
        client_id: client_id.to_string(),
        ms_refresh_token: refresh_token,
        ms_access_token: token.access_token,
        ms_expires_at: now + token.expires_in.saturating_sub(EXPIRY_SKEW_SECS),
        mc_access_token: chain.access_token,
        mc_expires_at: now + chain.expires_in.saturating_sub(EXPIRY_SKEW_SECS),
        username: chain.username,
        uuid: chain.uuid,
        xuid: chain.xuid,
        user_hash: chain.user_hash,
    };
    store_tokens(&tokens)?;
    Ok((tokens.session(), tokens))
}

struct ChainResult {
    access_token: String,
    expires_in: u64,
    username: String,
    uuid: String,
    xuid: String,
    user_hash: String,
}

async fn complete_chain(ms_access_token: &str) -> Result<ChainResult> {
    let xbl = xbox_authenticate(ms_access_token).await?;
    let xsts = xsts_authorize(&xbl.token).await?;
    let uhs = xsts
        .user_hash
        .clone()
        .ok_or_else(|| Error::msg("XSTS response did not include a user hash"))?;
    let xuid = xsts.xuid.clone().unwrap_or_default();

    let login = minecraft_login(&uhs, &xsts.token).await?;
    let profile = fetch_profile(&login.access_token).await?;

    Ok(ChainResult {
        access_token: login.access_token,
        expires_in: login.expires_in,
        username: profile.name,
        uuid: profile.id,
        xuid,
        user_hash: uhs,
    })
}

struct XboxToken {
    token: String,
    user_hash: Option<String>,
    xuid: Option<String>,
}

#[derive(Deserialize)]
struct RawXboxToken {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<Xui>,
}

#[derive(Deserialize)]
struct Xui {
    #[serde(default)]
    uhs: String,
    #[serde(default)]
    xid: Option<String>,
}

async fn xbox_authenticate(ms_access_token: &str) -> Result<XboxToken> {
    let rps = format!("d={ms_access_token}");
    let body = serde_json::json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": rps,
        },
        "RpsTicket": rps,
        "SiteName": "user.auth.xboxlive.com",
        "TokenType": "JWT",
    });
    let response = http::client()
        .post(XBL_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(Error::msg(format!(
            "Xbox Live authentication failed (HTTP {status}): {}",
            describe_auth_error(&text)
        )));
    }
    let raw: RawXboxToken =
        serde_json::from_str(&text).map_err(|e| Error::json("Xbox Live response", e))?;
    Ok(XboxToken {
        token: raw.token,
        user_hash: raw.claims.xui.first().map(|x| x.uhs.clone()),
        xuid: raw.claims.xui.first().and_then(|x| x.xid.clone()),
    })
}

async fn xsts_authorize(xbl_token: &str) -> Result<XboxToken> {
    let body = serde_json::json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token],
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    let response = http::client()
        .post(XSTS_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(code) = value["XErr"].as_u64() {
                return Err(Error::msg(xsts_error_message(code)));
            }
        }
        return Err(Error::msg(format!(
            "Xbox security token request failed (HTTP {status}): {}",
            describe_auth_error(&text)
        )));
    }
    let raw: RawXboxToken =
        serde_json::from_str(&text).map_err(|e| Error::json("XSTS response", e))?;
    Ok(XboxToken {
        token: raw.token,
        user_hash: raw.claims.xui.first().map(|x| x.uhs.clone()),
        xuid: raw.claims.xui.first().and_then(|x| x.xid.clone()),
    })
}

fn xsts_error_message(code: u64) -> String {
    let hint = match code {
        2148916233 => {
            "This Microsoft account has no Xbox profile. Sign in once at xbox.com, then retry."
        }
        2148916235 => "Xbox Live is not available in this account's country or region.",
        2148916236 | 2148916237 => {
            "This account needs adult verification before it can use Xbox Live."
        }
        2148916238 => "This is a child account. Add it to a Microsoft family group, then retry.",
        _ => "Xbox Live rejected this account.",
    };
    format!("{hint} (XErr {code})")
}

#[derive(Deserialize)]
struct McLoginResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

async fn minecraft_login(uhs: &str, xsts_token: &str) -> Result<McLoginResponse> {
    let body = serde_json::json!({
        "identityToken": format!("XBL3.0 x={uhs};{xsts_token}"),
    });
    let response = http::client()
        .post(MC_LOGIN_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::msg(
            "Minecraft services are rate limiting sign-in; wait a minute and retry.",
        ));
    }
    if !status.is_success() {
        return Err(Error::msg(format!(
            "Minecraft sign-in failed (HTTP {status}): {}",
            describe_auth_error(&text)
        )));
    }
    serde_json::from_str(&text).map_err(|e| Error::json("Minecraft login response", e))
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinecraftProfile {
    pub id: String,
    pub name: String,
}

pub async fn fetch_profile(mc_access_token: &str) -> Result<MinecraftProfile> {
    let response = http::client()
        .get(MC_PROFILE_URL)
        .bearer_auth(mc_access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?;
    match response.status() {
        reqwest::StatusCode::OK => {
            let text = response.text().await?;
            serde_json::from_str(&text).map_err(|e| Error::json("Minecraft profile", e))
        }
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Err(Error::AuthExpired)
        }
        reqwest::StatusCode::NOT_FOUND => Err(Error::NoOwnership),
        status => Err(Error::msg(format!(
            "Minecraft profile request failed with HTTP {status}"
        ))),
    }
}

/// Restore a stored session, refreshing silently if needed.
pub async fn restore(client_id: &str) -> Result<Option<AuthSession>> {
    let Some(tokens) = load_tokens()? else {
        return Ok(None);
    };
    if tokens.client_id != client_id {
        // Refresh tokens are bound to the client id that minted them.
        let _ = delete_tokens();
        return Ok(None);
    }

    if tokens.mc_expires_at > now_secs() + EXPIRY_SKEW_SECS {
        match fetch_profile(&tokens.mc_access_token).await {
            Ok(profile) => {
                let mut tokens = tokens;
                tokens.username = profile.name;
                tokens.uuid = profile.id;
                store_tokens(&tokens)?;
                return Ok(Some(tokens.session()));
            }
            Err(Error::AuthExpired) => {
                tracing::info!(target: "auth", "Minecraft token rejected; refreshing");
            }
            Err(Error::NoOwnership) => {
                let _ = delete_tokens();
                return Err(Error::NoOwnership);
            }
            Err(err) => {
                tracing::warn!(
                    target: "auth",
                    "could not validate the session ({err}); using the stored account"
                );
                return Ok(Some(tokens.session()));
            }
        }
    }
    refresh(&tokens).await.map(|(session, _)| Some(session))
}

pub async fn refresh(tokens: &StoredTokens) -> Result<(AuthSession, StoredTokens)> {
    let ms = refresh_ms_token(&tokens.client_id, &tokens.ms_refresh_token).await?;
    let chain = complete_chain(&ms.access_token).await?;
    let now = now_secs();
    let refreshed = StoredTokens {
        client_id: tokens.client_id.clone(),
        ms_refresh_token: ms
            .refresh_token
            .unwrap_or_else(|| tokens.ms_refresh_token.clone()),
        ms_access_token: ms.access_token,
        ms_expires_at: now + ms.expires_in.saturating_sub(EXPIRY_SKEW_SECS),
        mc_access_token: chain.access_token,
        mc_expires_at: now + chain.expires_in.saturating_sub(EXPIRY_SKEW_SECS),
        username: chain.username,
        uuid: chain.uuid,
        xuid: chain.xuid,
        user_hash: chain.user_hash,
    };
    store_tokens(&refreshed)?;
    Ok((refreshed.session(), refreshed))
}

fn keyring_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| {
        Error::msg(format!(
            "system keyring is unavailable ({e}); install/enable a Secret Service provider"
        ))
    })
}

pub fn store_tokens(tokens: &StoredTokens) -> Result<()> {
    let payload = serde_json::to_string(tokens).map_err(|e| Error::json("stored tokens", e))?;
    keyring_entry()?
        .set_password(&payload)
        .map_err(|e| Error::msg(format!("could not save the session to the keyring: {e}")))
}

pub fn load_tokens() -> Result<Option<StoredTokens>> {
    match keyring_entry()?.get_password() {
        Ok(payload) => match serde_json::from_str::<StoredTokens>(&payload) {
            Ok(tokens) => Ok(Some(tokens)),
            Err(err) => {
                tracing::warn!(target: "auth", "discarding unreadable keyring entry: {err}");
                let _ = delete_tokens();
                Ok(None)
            }
        },
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(Error::msg(format!(
            "could not read the session from the keyring: {err}"
        ))),
    }
}

pub fn delete_tokens() -> Result<()> {
    match keyring_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(Error::msg(format!(
            "could not remove the session from the keyring: {err}"
        ))),
    }
}

fn describe_auth_error(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    if let Some(description) = value["error_description"].as_str() {
        return trim_server_noise(description);
    }
    if let Some(error) = value["error"].as_str() {
        return error.to_string();
    }
    if let Some(message) = value["Message"].as_str() {
        return message.to_string();
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "no details from the server".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn trim_server_noise(message: &str) -> String {
    let cut = message
        .find(" Trace ID:")
        .or_else(|| message.find(" Correlation ID:"))
        .unwrap_or(message.len());
    message[..cut].trim().to_string()
}

fn now_secs() -> u64 {
    crate::bench::now_epoch_ms() / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_service_name_is_stable() {
        assert_eq!(KEYRING_SERVICE, "dev.kestrel.launcher");
    }

    #[test]
    fn keyring_roundtrip_when_available() {
        if keyring::Entry::store_status().is_err() {
            eprintln!("keyring store unavailable; skipping roundtrip");
            return;
        }
        let entry = keyring::Entry::new("dev.kestrel.launcher.test", "roundtrip").unwrap();
        entry.set_password("kestrel").unwrap();
        assert_eq!(entry.get_password().unwrap(), "kestrel");
        entry.delete_credential().unwrap();
    }

    #[test]
    fn device_code_is_never_serialized() {
        let info = DeviceCodeInfo {
            device_code: "secret-device-code".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://microsoft.com/link".into(),
            expires_in: 900,
            interval: 5,
            message: "link".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("secret-device-code"));
        assert!(json.contains("ABCD-EFGH"));
    }

    #[test]
    fn xsts_codes_have_friendly_messages() {
        assert!(xsts_error_message(2148916233).contains("xbox.com"));
        assert!(xsts_error_message(2148916238).contains("family"));
        assert!(xsts_error_message(999).contains("999"));
    }

    #[test]
    fn describe_error_extracts_description() {
        let body = r#"{"error":"invalid_grant","error_description":"AADSTS70008: expired"}"#;
        assert_eq!(describe_auth_error(body), "AADSTS70008: expired");
        assert_eq!(describe_auth_error("plain"), "plain");
        let noisy = r#"{"error_description":"AADSTS70008: expired Trace ID: abc Correlation ID: def Timestamp: now"}"#;
        assert_eq!(describe_auth_error(noisy), "AADSTS70008: expired");
    }

    #[test]
    fn session_summary_matches_state_shape() {
        let session = AuthSession {
            username: "Steve".into(),
            uuid: "abc".into(),
            access_token: "t".into(),
            user_type: "msa".into(),
            xuid: "1".into(),
            client_id: "c".into(),
            expires_at: 0,
        };
        assert_eq!(session.summary().username, "Steve");
        assert_eq!(session.summary().kind, AccountKind::Microsoft);
    }
}
