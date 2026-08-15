use crate::models::Killmail;
use reqwest::blocking::Client;

pub fn post(mail: &Killmail) -> Result<String, String> {
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
