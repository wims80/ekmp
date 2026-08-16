use crate::{models::Character, persistence::secrets};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, TryRngCore};
use reqwest::{blocking::Client, Url};
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

pub fn authenticate(cancelled: &AtomicBool) -> Result<Character, String> {
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
    let listener =
        TcpListener::bind("127.0.0.1:17842").map_err(|e| format!("Callback unavailable: {e}"))?;
    open::that(url.as_str())
        .map_err(|_| "Could not open browser; use the authorization URL manually".to_string())?;
    let code = receive_callback(listener, &state, cancelled)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Character connection cancelled".into());
    }
    exchange_code(&verifier, &code)
}

pub fn access_token(c: &Character) -> Result<String, String> {
    let refresh_token = match &c.refresh_token {
        Some(token) => token.clone(),
        None => secrets::load_refresh_token(c.id).map_err(|secure_error| {
            format!(
                "Could not read the refresh token from the system credential store: {secure_error}. Re-authenticate this character."
            )
        })?,
    };
    let response = Client::new()
        .post(format!("{SSO}/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .map_err(|e| format!("Token refresh failed: {e}"))?;
    let token: Token = decode_response(response, "Token refresh")?;
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
    if callback
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v != expected_state)
        .unwrap_or(true)
    {
        return Err("OAuth state validation failed".into());
    }
    let code = callback
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .ok_or("Authorization failed")?;
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nAuthorization complete. You can close this window.");
    Ok(code)
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

fn exchange_code(verifier: &str, code: &str) -> Result<Character, String> {
    let response = Client::new()
        .post(format!("{SSO}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .map_err(|e| format!("Token request failed: {e}"))?;
    let token: Token = decode_response(response, "Token request")?;
    let response = Client::new()
        .get("https://login.eveonline.com/oauth/verify")
        .bearer_auth(token.access_token)
        .send()
        .map_err(|e| format!("Character verification request failed: {e}"))?;
    let verify: Verify = decode_response(response, "Character verification")?;
    Ok(Character {
        id: verify.character_id,
        name: verify.character_name,
        refresh_token: Some(token.refresh_token),
        corporation_id: None,
        corporation_name: None,
    })
}

fn decode_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
    operation: &str,
) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .map_err(|e| format!("{operation} response could not be read: {e}"))?;
    if !status.is_success() {
        return Err(format!("{operation} failed ({status}): {body}"));
    }
    serde_json::from_str(&body)
        .map_err(|e| format!("{operation} returned invalid JSON ({e}): {body}"))
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
