use crate::{
    integrations::{http, ApiResult},
    models::Character,
    persistence::secrets,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, TryRngCore};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
const CALLBACK: &str = "http://127.0.0.1:17842/callback";
// EVE client IDs are public identifiers. Replace this with the client ID for the
// application whose callback URL is CALLBACK; PKCE means no client secret is needed.
const CLIENT_ID: &str = "5df72c2c20ce4c70ad2863766e130d33";
const SSO: &str = "https://login.eveonline.com/v2/oauth";
const SCOPE: &str = "esi-killmails.read_killmails.v1";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How the authorization code gets back to ekmp after the user signs in.
#[derive(Clone, Copy)]
pub(crate) enum AuthFlow<'a> {
    /// Wait for the browser's redirect on the local callback, optionally opening the
    /// browser. Needs the browser on this machine or an SSH tunnel to the callback port.
    Loopback { open_browser: bool },
    /// Read the redirect URL that the user copies from the browser on any machine. The
    /// reader returns `None` when cancelled.
    Paste(&'a dyn Fn() -> Option<String>),
}

pub fn authenticate(
    cancelled: &AtomicBool,
    flow: AuthFlow,
    on_authorization_url: &dyn Fn(&str),
) -> ApiResult<Character> {
    let mut random = OsRng;
    let mut state_bytes = [0_u8; 32];
    let mut verifier_bytes = [0_u8; 32];
    random
        .try_fill_bytes(&mut state_bytes)
        .and_then(|()| random.try_fill_bytes(&mut verifier_bytes))
        .map_err(|e| format!("Secure random number generation failed: {e}"))?;
    let state = URL_SAFE_NO_PAD.encode(state_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&format!("{SSO}/authorize")).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", CALLBACK)
        .append_pair("client_id", CLIENT_ID)
        .append_pair("scope", SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let code = match flow {
        AuthFlow::Loopback { open_browser } => {
            let listener = TcpListener::bind("127.0.0.1:17842")
                .map_err(|e| format!("Callback unavailable: {e}"))?;
            on_authorization_url(url.as_str());
            if open_browser {
                open::that(url.as_str()).map_err(|_| {
                    "Could not open the browser; use --no-browser to open the authorization URL manually"
                        .to_string()
                })?;
            }
            receive_callback(listener, &state, cancelled)?
        }
        AuthFlow::Paste(read_pasted_url) => {
            on_authorization_url(url.as_str());
            let pasted = read_pasted_url().ok_or("Character connection cancelled")?;
            let callback = Url::parse(pasted.trim())
                .map_err(|_| "The pasted text is not a URL; paste the full address bar")?;
            if !is_callback(&callback) {
                return Err(format!("The pasted URL must start with {CALLBACK}").into());
            }
            callback_code(&callback, &state)?
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        return Err("Character connection cancelled".into());
    }
    exchange_code(&verifier, &code, cancelled)
}

pub fn access_token(
    c: &mut Character,
    on_character_updated: &mut dyn FnMut(&Character) -> ApiResult<()>,
) -> ApiResult<String> {
    let refresh_token = match &c.refresh_token {
        Some(token) => token.clone(),
        None => secrets::load_refresh_token(c.id).map_err(|secure_error| {
            format!(
                "Could not read the refresh token from the system credential store: {secure_error}. Re-authenticate this character."
            )
        })?,
    };
    let response = http::client()?
        .post(format!("{SSO}/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .map_err(|error| http::transport_error("Token refresh", &error))?;
    let token: Token = http::decode_json(response, "Token refresh")?;
    if token.refresh_token != refresh_token {
        if c.refresh_token.is_some() {
            c.refresh_token = Some(token.refresh_token.clone());
        } else if secrets::save_refresh_token(c.id, &token.refresh_token).is_err() {
            // If the credential store became unavailable, preserve the rotated
            // token through the normal private JSON fallback path.
            c.refresh_token = Some(token.refresh_token.clone());
        }
        on_character_updated(c)?;
    }
    Ok(token.access_token)
}

fn receive_callback(
    listener: TcpListener,
    expected_state: &str,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    receive_callback_until(
        listener,
        expected_state,
        cancelled,
        Instant::now() + CALLBACK_TIMEOUT,
    )
}

fn receive_callback_until(
    listener: TcpListener,
    expected_state: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not monitor the authorization callback: {error}"))?;
    let mut stream = loop {
        check_callback_wait(cancelled, deadline)?;
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(CALLBACK_POLL_INTERVAL);
            }
            Err(error) => return Err(format!("Authorization callback failed: {error}")),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("Could not configure the authorization callback: {error}"))?;
    let mut buffer = [0; 4096];
    let size = stream.read(&mut buffer).map_err(|e| e.to_string())?;
    let request = String::from_utf8_lossy(&buffer[..size]);
    let target = request
        .split_whitespace()
        .nth(1)
        .ok_or("Invalid callback")?;
    let callback = Url::parse(&format!("http://localhost{target}")).map_err(|e| e.to_string())?;
    let code = callback_code(&callback, expected_state)?;
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nAuthorization complete. You can close this window.");
    Ok(code)
}

/// Whether `url` is the registered callback, apart from its query.
fn is_callback(url: &Url) -> bool {
    let expected = Url::parse(CALLBACK).expect("CALLBACK is a valid URL");
    url.scheme() == expected.scheme()
        && url.host() == expected.host()
        && url.port_or_known_default() == expected.port_or_known_default()
        && url.path() == expected.path()
}

/// Validates the callback's OAuth `state` and returns its authorization code.
fn callback_code(callback: &Url, expected_state: &str) -> Result<String, String> {
    let parameter = |name: &str| {
        callback
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if parameter("state").as_deref() != Some(expected_state) {
        return Err("OAuth state validation failed; start the connection again".into());
    }
    if let Some(error) = parameter("error") {
        return Err(format!("EVE SSO refused the authorization: {error}"));
    }
    parameter("code").ok_or_else(|| "Authorization failed: the callback has no code".into())
}

fn check_callback_wait(cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Character connection cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("Character connection timed out; start the connection again".into());
    }
    Ok(())
}

fn exchange_code(verifier: &str, code: &str, cancelled: &AtomicBool) -> ApiResult<Character> {
    let client = http::client()?;
    let response = client
        .post(format!("{SSO}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .map_err(|error| http::transport_error("Token request", &error))?;
    let token: Token = http::decode_json(response, "Token request")?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Character connection cancelled".into());
    }
    let response = client
        .get("https://login.eveonline.com/oauth/verify")
        .bearer_auth(token.access_token)
        .send()
        .map_err(|error| http::transport_error("Character verification request", &error))?;
    let verify: Verify = http::decode_json(response, "Character verification")?;
    Ok(Character {
        id: verify.character_id,
        name: verify.character_name,
        refresh_token: Some(token.refresh_token),
        corporation_id: None,
        corporation_name: None,
    })
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: String,
}
#[derive(Deserialize)]
struct Verify {
    #[serde(rename = "CharacterID")]
    character_id: u64,
    #[serde(rename = "CharacterName")]
    character_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callback(query: &str) -> Url {
        Url::parse(&format!("{CALLBACK}?{query}")).unwrap()
    }

    #[test]
    fn callback_code_requires_the_expected_state() {
        assert_eq!(
            callback_code(&callback("code=abc&state=expected"), "expected"),
            Ok("abc".into())
        );
        for query in ["code=abc&state=other", "code=abc"] {
            assert!(callback_code(&callback(query), "expected")
                .unwrap_err()
                .contains("state validation failed"));
        }
    }

    #[test]
    fn callback_without_code_or_with_an_sso_error_is_rejected() {
        assert!(callback_code(&callback("state=expected"), "expected")
            .unwrap_err()
            .contains("no code"));
        assert!(
            callback_code(&callback("error=access_denied&state=expected"), "expected")
                .unwrap_err()
                .contains("access_denied")
        );
    }

    #[test]
    fn only_the_registered_callback_is_accepted() {
        assert!(is_callback(&callback("code=abc&state=s")));
        for other in [
            "https://127.0.0.1:17842/callback",
            "http://localhost:17842/callback",
            "http://127.0.0.1:17843/callback",
            "http://127.0.0.1:17842/other",
            "http://attacker.example/callback",
        ] {
            assert!(!is_callback(&Url::parse(other).unwrap()), "{other}");
        }
    }

    #[test]
    fn callback_wait_can_be_cancelled() {
        let cancelled = AtomicBool::new(true);

        let error =
            check_callback_wait(&cancelled, Instant::now() + Duration::from_secs(60)).unwrap_err();

        assert_eq!(error, "Character connection cancelled");
    }

    #[test]
    fn callback_wait_times_out() {
        let cancelled = AtomicBool::new(false);

        let error = check_callback_wait(&cancelled, Instant::now()).unwrap_err();

        assert_eq!(
            error,
            "Character connection timed out; start the connection again"
        );
    }
}
