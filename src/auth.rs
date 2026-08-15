use crate::models::Character;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
};
use url::Url;

const CALLBACK: &str = "http://127.0.0.1:17842/callback";
const SSO: &str = "https://login.eveonline.com/v2/oauth";
const SCOPE: &str = "esi-killmails.read_killmails.v1";

pub fn callback_url() -> &'static str {
    CALLBACK
}

pub fn authenticate(client_id: &str) -> Result<Character, String> {
    let state = uuid::Uuid::new_v4().to_string();
    let mut verifier_bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&format!("{SSO}/authorize")).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", CALLBACK)
        .append_pair("client_id", client_id)
        .append_pair("scope", SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let listener =
        TcpListener::bind("127.0.0.1:17842").map_err(|e| format!("Callback unavailable: {e}"))?;
    open::that(url.as_str())
        .map_err(|_| "Could not open browser; use the authorization URL manually".to_string())?;
    let code = receive_callback(listener, &state)?;
    exchange_code(client_id, &verifier, &code)
}

pub fn access_token(c: &Character, client_id: &str) -> Result<String, String> {
    let response = Client::new()
        .post(&format!("{SSO}/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", c.refresh_token.as_str()),
            ("client_id", client_id),
        ])
        .send()
        .map_err(|e| format!("Token refresh failed: {e}"))?;
    let token: Token = decode_response(response, "Token refresh")?;
    Ok(token.access_token)
}

fn receive_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
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

fn exchange_code(client_id: &str, verifier: &str, code: &str) -> Result<Character, String> {
    let response = Client::new()
        .post(&format!("{SSO}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
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
        refresh_token: token.refresh_token,
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
