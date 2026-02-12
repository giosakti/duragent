//! Anthropic OAuth PKCE flow.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

// ============================================================================
// Constants
// ============================================================================

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

// ============================================================================
// Types
// ============================================================================

/// OAuth tokens returned from token exchange or refresh.
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp in seconds when the access token expires.
    pub expires_at: i64,
}

// ============================================================================
// Public API
// ============================================================================

/// Generate a PKCE code verifier, challenge, and OAuth state parameter.
///
/// Returns `(verifier, challenge, state)` where:
/// - `verifier` is 32 random bytes encoded as base64url
/// - `challenge` is SHA-256 of verifier encoded as base64url
/// - `state` is a separate 32 random bytes encoded as base64url (CSRF protection)
pub fn generate_pkce() -> (String, String, String) {
    use rand::Rng;

    let mut rng = rand::rng();

    let mut verifier_bytes = [0u8; 32];
    rng.fill(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    let mut state_bytes = [0u8; 32];
    rng.fill(&mut state_bytes);
    let state = URL_SAFE_NO_PAD.encode(state_bytes);

    (verifier, challenge, state)
}

/// Build the authorization URL for the browser.
///
/// Uses a separate state parameter for CSRF protection (not the PKCE verifier).
pub fn build_authorize_url(challenge: &str, state: &str) -> String {
    let mut url = url::Url::parse(AUTHORIZE_URL).expect("valid authorize URL");
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    url.to_string()
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<OAuthTokens> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": CLIENT_ID,
        "code": code,
        "state": state,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });

    let response = client
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await
        .context("sending token exchange request")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("token exchange failed ({}): {}", status, text);
    }

    parse_token_response(response).await
}

/// Refresh an expired access token.
pub async fn refresh_token(client: &reqwest::Client, refresh: &str) -> Result<OAuthTokens> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": refresh,
    });

    let response = client
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await
        .context("sending token refresh request")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("token refresh failed ({}): {}", status, text);
    }

    parse_token_response(response).await
}

// ============================================================================
// Private helpers
// ============================================================================

/// Validate that the returned state matches the expected state.
pub fn validate_state(returned: &str, expected: &str) -> Result<()> {
    if returned != expected {
        bail!("OAuth state mismatch (possible CSRF attack)");
    }
    Ok(())
}

async fn parse_token_response(response: reqwest::Response) -> Result<OAuthTokens> {
    let body: serde_json::Value = response.json().await.context("parsing token response")?;

    let access_token = body["access_token"]
        .as_str()
        .context("missing access_token")?
        .to_string();
    let refresh_token = body["refresh_token"]
        .as_str()
        .context("missing refresh_token")?
        .to_string();
    let expires_in = body["expires_in"].as_i64().unwrap_or(3600);

    let expires_at = chrono::Utc::now().timestamp() + expires_in;

    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_pkce_state_differs_from_verifier() {
        let (verifier, _challenge, state) = generate_pkce();
        assert_ne!(verifier, state, "state must be independent of verifier");
    }

    #[test]
    fn authorize_url_does_not_leak_verifier() {
        let (verifier, challenge, state) = generate_pkce();
        let url = build_authorize_url(&challenge, &state);
        assert!(!url.contains(&verifier), "URL must not contain verifier");
        assert!(url.contains(&state), "URL must contain state");
        assert!(url.contains(&challenge), "URL must contain challenge");
    }

    #[test]
    fn validate_state_rejects_mismatch() {
        assert!(validate_state("abc", "xyz").is_err());
    }

    #[test]
    fn validate_state_accepts_match() {
        assert!(validate_state("abc", "abc").is_ok());
    }
}
