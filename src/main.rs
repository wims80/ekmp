use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use eframe::egui;
use rand::RngCore;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use url::Url;

const CALLBACK: &str = "http://127.0.0.1:17842/callback";
const ESI: &str = "https://esi.evetech.net/latest";
const SSO: &str = "https://login.eveonline.com/v2/oauth";
const SCOPE: &str = "esi-killmails.read_killmails.v1";

#[derive(Default, Serialize, Deserialize)]
struct Store {
    client_id: String,
    characters: Vec<Character>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Character {
    id: u64,
    name: String,
    refresh_token: String,
}

#[derive(Clone)]
struct Killmail {
    id: u64,
    hash: String,
    character: String,
    victim_id: Option<u64>,
    victim: String,
    ship: String,
    time: String,
}

enum AppEvent {
    Character(Character),
    Killmails(Vec<Killmail>),
}

struct App {
    store: Store,
    killmails: Vec<Killmail>,
    status: String,
    auth_rx: Option<Receiver<Result<AppEvent, String>>>,
    post_rx: Option<Receiver<Result<String, String>>>,
    loading: bool,
}

impl App {
    fn new() -> Self {
        let store = load_store();
        Self {
            store,
            killmails: Vec::new(),
            status: "Ready".into(),
            auth_rx: None,
            post_rx: None,
            loading: false,
        }
    }

    fn persist(&self) -> Result<(), String> {
        let path = store_path()?;
        let data = serde_json::to_vec_pretty(&self.store).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())
    }

    fn begin_auth(&mut self) {
        if self.auth_rx.is_some() {
            self.status = "An authentication or load operation is already in progress".into();
            return;
        }
        if self.store.client_id.trim().is_empty() {
            self.status = "Enter the ESI client ID first".into();
            return;
        }
        let state = uuid::Uuid::new_v4().to_string();
        let mut verifier_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut verifier_bytes);
        let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = Url::parse(&format!("{SSO}/authorize")).unwrap();
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", CALLBACK)
            .append_pair("client_id", &self.store.client_id)
            .append_pair("scope", SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        let (tx, rx) = mpsc::channel();
        let client_id = self.store.client_id.clone();
        thread::spawn(move || {
            let result = receive_callback(&state)
                .and_then(|code| exchange_code(&client_id, &verifier, &code))
                .map(AppEvent::Character);
            let _ = tx.send(result);
        });
        if open::that(url.as_str()).is_err() {
            self.status = "Could not open browser; use the authorization URL manually".into();
        } else {
            self.status = "Authorize the character in your browser...".into();
        }
        self.auth_rx = Some(rx);
        self.loading = true;
    }

    fn refresh_killmails(&mut self) {
        if self.auth_rx.is_some() {
            self.status = "An authentication or load operation is already in progress".into();
            return;
        }
        if self.store.characters.is_empty() {
            self.status = "Authenticate at least one character first".into();
            return;
        }
        let chars = self.store.characters.clone();
        self.status = "Loading recent killmails...".into();
        self.loading = true;
        let (tx, rx) = mpsc::channel();
        let client_id = self.store.client_id.clone();
        thread::spawn(move || {
            let result = load_all_killmails(&chars, &client_id).map(AppEvent::Killmails);
            let _ = tx.send(result);
        });
        self.auth_rx = Some(rx);
    }

    fn post_killmail_async(&mut self, mail: &Killmail) {
        if self.post_rx.is_some() {
            self.status = "A zKillboard submission is already in progress".into();
            return;
        }
        let mail = mail.clone();
        let mail_id = mail.id;
        let request_mail = mail.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(post_killmail(&request_mail));
        });
        self.post_rx = Some(rx);
        self.status = format!("Submitting killmail {} to zKillboard...", mail_id);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.auth_rx {
            if let Ok(result) = rx.try_recv() {
                self.auth_rx = None;
                self.loading = false;
                match result {
                    Ok(AppEvent::Character(c)) => {
                        self.store.characters.retain(|old| old.id != c.id);
                        self.store.characters.push(c);
                        self.status = "Character authenticated".into();
                        let _ = self.persist();
                    }
                    Ok(AppEvent::Killmails(killmails)) => {
                        self.killmails = killmails;
                        self.status = "Killmails loaded".into();
                    }
                    Err(e) => self.status = e,
                }
            }
        }
        if let Some(rx) = &self.post_rx {
            if let Ok(result) = rx.try_recv() {
                self.post_rx = None;
                self.status = match result {
                    Ok(message) => message,
                    Err(error) => format!("zKillboard submission failed: {error}"),
                };
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("akmp");
            ui.label("EVE Online killmail reporter");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("ESI client ID");
                ui.text_edit_singleline(&mut self.store.client_id);
            });
            if ui.button("Authenticate character").clicked() {
                let _ = self.persist();
                self.begin_auth();
            }
            ui.label("Register this callback URL in your EVE developer application:");
            ui.monospace(CALLBACK);
            ui.separator();
            ui.heading("Authenticated characters");
            for c in &self.store.characters {
                ui.label(format!("{} ({})", c.name, c.id));
            }
            if ui.button("Load recent killmails").clicked() {
                self.refresh_killmails();
            }
            ui.separator();
            ui.heading("Recent killmails");
            if self.killmails.is_empty() {
                ui.label("No killmails loaded.");
            }
            let authenticated_ids: Vec<u64> = self.store.characters.iter().map(|c| c.id).collect();
            let mut post_id = None;
            for mail in &self.killmails {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} | killed: {} | ship: {} | date: {} | source: {}",
                        mail.id, mail.victim, mail.ship, mail.time, mail.character
                    ));
                    let own_character = mail
                        .victim_id
                        .is_some_and(|id| authenticated_ids.contains(&id));
                    if !own_character
                        && ui
                            .add_enabled(
                                self.post_rx.is_none(),
                                egui::Button::new("Post to zKillboard"),
                            )
                            .clicked()
                    {
                        post_id = Some(mail.id);
                    }
                    if own_character {
                        ui.label("Not postable: authenticated character");
                    }
                });
            }
            if let Some(id) = post_id {
                if let Some(mail) = self.killmails.iter().find(|mail| mail.id == id).cloned() {
                    self.post_killmail_async(&mail);
                }
            }
            ui.separator();
            ui.label(format!("Status: {}", self.status));
        });
        if self.auth_rx.is_some() || self.post_rx.is_some() || self.loading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn receive_callback(expected_state: &str) -> Result<String, String> {
    let listener =
        TcpListener::bind("127.0.0.1:17842").map_err(|e| format!("Callback unavailable: {e}"))?;
    listener.set_nonblocking(false).ok();
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
    let token_response = Client::new()
        .post(&format!("{SSO}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .map_err(|e| format!("Token request failed: {e}"))?;
    let token: Token = decode_response(token_response, "Token request")?;

    let verify_response = Client::new()
        .get("https://login.eveonline.com/oauth/verify")
        .bearer_auth(token.access_token)
        .send()
        .map_err(|e| format!("Character verification request failed: {e}"))?;
    let verify: Verify = decode_response(verify_response, "Character verification")?;
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
#[derive(Deserialize)]
struct Recent {
    killmail_id: u64,
    killmail_hash: String,
}

#[derive(Deserialize)]
struct KillmailDetail {
    killmail_time: String,
    victim: Victim,
}

#[derive(Deserialize)]
struct Victim {
    character_id: Option<u64>,
    ship_type_id: Option<u64>,
}

fn access_token(c: &Character, client_id: &str) -> Result<String, String> {
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
fn load_all_killmails(chars: &[Character], client_id: &str) -> Result<Vec<Killmail>, String> {
    let client = Client::new();
    let mut result = Vec::new();
    for c in chars {
        let response: Vec<Recent> = client
            .get(format!("{ESI}/characters/{}/killmails/recent/", c.id))
            .bearer_auth(access_token(c, client_id)?)
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        for recent in response {
            let detail: KillmailDetail = client
                .get(format!(
                    "{ESI}/killmails/{}/{}",
                    recent.killmail_id, recent.killmail_hash
                ))
                .send()
                .map_err(|e| format!("Killmail {} request failed: {e}", recent.killmail_id))?
                .error_for_status()
                .map_err(|e| format!("Killmail {} request failed: {e}", recent.killmail_id))?
                .json()
                .map_err(|e| format!("Killmail {} response invalid: {e}", recent.killmail_id))?;
            result.push((recent, detail, c.name.clone()));
        }
    }

    result
        .into_iter()
        .map(|(recent, detail, character)| {
            let victim = detail
                .victim
                .character_id
                .map(|id| resolve_character_name(&client, id))
                .transpose()?
                .unwrap_or_else(|| "Unknown character".into());
            let ship = detail
                .victim
                .ship_type_id
                .map(|id| resolve_type_name(&client, id))
                .transpose()?
                .unwrap_or_else(|| "Unknown ship".into());
            Ok(Killmail {
                id: recent.killmail_id,
                hash: recent.killmail_hash,
                character,
                victim_id: detail.victim.character_id,
                victim,
                ship,
                time: detail.killmail_time,
            })
        })
        .collect()
}

fn resolve_character_name(client: &Client, id: u64) -> Result<String, String> {
    #[derive(Deserialize)]
    struct CharacterInfo {
        name: String,
    }
    let response = client
        .get(format!("{ESI}/characters/{id}/"))
        .send()
        .map_err(|e| format!("Character name lookup failed for {id}: {e}"))?;
    let info: CharacterInfo = response
        .error_for_status()
        .map_err(|e| format!("Character name lookup failed for {id}: {e}"))?
        .json()
        .map_err(|e| format!("Character name response invalid for {id}: {e}"))?;
    Ok(info.name)
}

fn resolve_type_name(client: &Client, id: u64) -> Result<String, String> {
    #[derive(Deserialize)]
    struct TypeInfo {
        name: String,
    }
    let response = client
        .get(format!("{ESI}/universe/types/{id}/"))
        .send()
        .map_err(|e| format!("Ship name lookup failed for type {id}: {e}"))?;
    let info: TypeInfo = response
        .error_for_status()
        .map_err(|e| format!("Ship name lookup failed for type {id}: {e}"))?
        .json()
        .map_err(|e| format!("Ship name response invalid for type {id}: {e}"))?;
    Ok(info.name)
}
fn post_killmail(mail: &Killmail) -> Result<String, String> {
    let response = Client::new()
        .post(format!(
            "https://zkillboard.com/api/killmail/add/{}/{}/",
            mail.id, mail.hash
        ))
        .header("User-Agent", "akmp/0.1 (EVE killmail reporter)")
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    Ok(format!("zKillboard response: {}", response.status()))
}
fn store_path() -> Result<PathBuf, String> {
    dirs_path().map(|p| p.join("akmp.json"))
}
fn dirs_path() -> Result<PathBuf, String> {
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join(".config").join("akmp"))
        .ok_or("HOME is not set")?;
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
fn load_store() -> Store {
    store_path()
        .ok()
        .and_then(|p| fs::read(p).ok())
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default()
}

fn main() -> eframe::Result {
    eframe::run_native(
        "akmp",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(App::new()))),
    )
}
