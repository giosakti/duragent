//! Login command for authenticating with LLM providers.

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};

use agnx::auth::anthropic_oauth;
use agnx::auth::credentials::{AuthCredential, AuthStorage};

/// Run the login flow for a provider.
pub async fn run(provider: &str) -> Result<()> {
    match provider {
        "anthropic" => login_anthropic().await,
        _ => bail!(
            "Unsupported provider: '{}'. Currently only 'anthropic' is supported.",
            provider
        ),
    }
}

async fn login_anthropic() -> Result<()> {
    println!("Authenticating with Anthropic...");
    println!();

    // Generate PKCE
    let (verifier, challenge) = anthropic_oauth::generate_pkce();

    // Use the full verifier as the state parameter (same as pi-mono)
    let url = anthropic_oauth::build_authorize_url(&challenge, &verifier);

    println!("Open this URL in your browser to authorize:");
    println!();
    println!("  {}", url);
    println!();
    println!("After authorizing, you'll be redirected to a page showing a code.");
    println!("Paste the full code (including the #state part) below:");
    println!();

    // Read code from stdin
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    eprint!("> ");
    let input = lines
        .next_line()
        .await?
        .unwrap_or_default()
        .trim()
        .to_string();

    if input.is_empty() {
        bail!("No code provided. Aborting.");
    }

    // Parse code#state format
    let (code, state) = if let Some(pos) = input.find('#') {
        (&input[..pos], &input[pos + 1..])
    } else {
        (input.as_str(), "")
    };

    println!();
    println!("Exchanging code for tokens...");

    let client = reqwest::Client::new();
    let tokens = anthropic_oauth::exchange_code(&client, code, state, &verifier).await?;

    // Save credentials
    let auth_path = AuthStorage::default_path();
    let mut storage = AuthStorage::load(&auth_path)?;
    storage.set_anthropic(AuthCredential::OAuth {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: tokens.expires_at,
    });
    storage.save(&auth_path)?;

    println!(
        "Authentication successful! Credentials saved to {}",
        auth_path.display()
    );
    println!();
    println!("You can now use Anthropic models without setting ANTHROPIC_API_KEY.");
    println!("OAuth credentials take precedence over API key (if set).");

    Ok(())
}
