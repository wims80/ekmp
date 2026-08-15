use keyring::v1::Entry;

const SERVICE: &str = "EVE Killmail Publisher";

fn account(character_id: u64) -> String {
    format!("eve-character-{character_id}")
}

fn entry(character_id: u64) -> Result<Entry, String> {
    Entry::new(SERVICE, &account(character_id)).map_err(|error| error.to_string())
}

pub fn save_refresh_token(character_id: u64, token: &str) -> Result<(), String> {
    entry(character_id)?
        .set_password(token)
        .map_err(|error| error.to_string())
}

pub fn load_refresh_token(character_id: u64) -> Result<String, String> {
    entry(character_id)?
        .get_password()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_account_is_stable_and_character_specific() {
        assert_eq!(account(42), "eve-character-42");
        assert_ne!(account(42), account(43));
    }
}
