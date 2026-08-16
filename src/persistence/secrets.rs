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

pub fn delete_refresh_token(character_id: u64) -> Result<(), String> {
    entry(character_id)?
        .delete_credential()
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

    #[cfg(windows)]
    #[test]
    #[ignore = "writes a temporary entry to the user's Windows Credential Manager"]
    fn windows_credential_manager_round_trip() {
        let character_id = u64::MAX - u64::from(std::process::id());
        let token = "ekmp-windows-credential-manager-test";
        let _cleanup = CredentialCleanup(character_id);

        save_refresh_token(character_id, token)
            .expect("Windows Credential Manager should accept a test credential");
        assert_eq!(
            load_refresh_token(character_id)
                .expect("Windows Credential Manager should return the test credential"),
            token
        );
    }

    #[cfg(windows)]
    struct CredentialCleanup(u64);

    #[cfg(windows)]
    impl Drop for CredentialCleanup {
        fn drop(&mut self) {
            let _ = delete_refresh_token(self.0);
        }
    }
}
