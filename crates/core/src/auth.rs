//! Account sessions.
//!
//! Stage 2 only needs a placeholder session to exercise the launch pipeline.
//! The real Microsoft device-code chain lands in stage 3; until then a token is
//! never persisted and the dev session is gated behind `KESTREL_DEV_AUTH=1`.
//! This is a development aid, not an offline-mode feature.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: String,
    pub xuid: String,
    pub client_id: String,
}

impl AuthSession {
    pub fn uuid_undashed(&self) -> String {
        self.uuid.replace('-', "")
    }

    pub fn dev_enabled() -> bool {
        matches!(
            std::env::var("KESTREL_DEV_AUTH").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        )
    }

    /// Placeholder identity for smoke-testing the launch pipeline against the
    /// title screen. Online servers will reject this token by design.
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
        })
    }
}
