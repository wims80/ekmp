#[cfg(feature = "gui")]
use super::CredentialMigrationResult;
use super::{
    timing::{active_api_cooldown, check_cancelled},
    Cancellation, Core, CoreError, CoreResult, RemoveCharacterResult,
};
use crate::{
    integrations::auth::AuthFlow, killmail::remove_killmails_for_removed_character,
    models::Character,
};

impl Core {
    pub(crate) fn authenticate(
        &self,
        cancelled: &Cancellation,
        flow: AuthFlow,
        on_authorization_url: &dyn Fn(&str),
    ) -> CoreResult<Character> {
        let mut locked = self.begin_operation()?;
        check_cancelled(cancelled)?;
        self.progress("Waiting for EVE authorization");
        let mut character = self
            .backend
            .authenticate(cancelled.as_atomic(), flow, on_authorization_url)
            .map_err(|error| {
                if cancelled.is_cancelled() {
                    CoreError::Cancelled
                } else {
                    CoreError::Operational(format!("authentication failed: {error}"))
                }
            })?;
        check_cancelled(cancelled)?;
        if active_api_cooldown(locked.store(), "esi").is_none() {
            if let Err(error) = self
                .backend
                .refresh_character_affiliation(&mut character, cancelled.as_atomic())
            {
                self.progress(format!(
                    "Character authenticated, but corporation lookup failed: {error}"
                ));
            }
            self.absorb_api_observations(locked.store_mut());
        } else {
            self.progress(
                "Character authenticated; corporation lookup is deferred by an API cooldown",
            );
        }
        check_cancelled(cancelled)?;
        if let Some(previous) = locked
            .store()
            .characters
            .iter()
            .find(|previous| previous.id == character.id)
        {
            if character.corporation_id.is_none() {
                character.corporation_id = previous.corporation_id;
                character
                    .corporation_name
                    .clone_from(&previous.corporation_name);
            } else if character.corporation_id == previous.corporation_id
                && character.corporation_name.is_none()
            {
                character
                    .corporation_name
                    .clone_from(&previous.corporation_name);
            }
        }
        locked
            .store_mut()
            .characters
            .retain(|old| old.id != character.id);
        locked.store_mut().characters.push(character.clone());
        // First make the new credential durable as a JSON fallback. A keyring update cannot
        // otherwise be rolled back safely when replacing an existing character credential.
        locked.persist()?;
        if let Some(token) = character.refresh_token.as_deref() {
            if self.backend.save_refresh_token(character.id, token).is_ok() {
                character.refresh_token = None;
                if let Some(stored) = locked
                    .store_mut()
                    .characters
                    .iter_mut()
                    .find(|stored| stored.id == character.id)
                {
                    stored.refresh_token = None;
                }
                // If this persistence step fails, the previously persisted JSON fallback is
                // still intact and can recover the newly saved keyring credential.
                locked.persist()?;
            }
        }
        self.changed();
        Ok(character)
    }

    pub(crate) fn remove_character(&self, id: u64) -> CoreResult<RemoveCharacterResult> {
        let mut locked = self.begin_operation()?;
        let character = locked
            .store()
            .characters
            .iter()
            .find(|character| character.id == id)
            .cloned()
            .ok_or_else(|| {
                CoreError::Operational(format!("character {id} is not authenticated"))
            })?;
        locked.store_mut().characters.retain(|entry| entry.id != id);
        let store_view = locked.store().clone();
        let removed_killmails = remove_killmails_for_removed_character(
            &store_view,
            &mut locked.store_mut().cached_killmails,
            id,
        );
        locked.persist()?;
        let credential_warning = if character.uses_json_refresh_token_fallback() {
            None
        } else {
            self.backend
                .delete_refresh_token(id)
                .err()
                .map(|error| error.to_string())
        };
        self.changed();
        Ok(RemoveCharacterResult {
            id,
            name: character.name,
            removed_killmails,
            credential_warning,
        })
    }

    #[cfg(feature = "gui")]
    pub(crate) fn migrate_refresh_tokens(&self) -> CoreResult<CredentialMigrationResult> {
        let mut locked = self.begin_operation()?;
        let candidates = locked
            .store()
            .characters
            .iter()
            .filter_map(|character| {
                character
                    .refresh_token
                    .clone()
                    .map(|token| (character.id, character.name.clone(), token))
            })
            .collect::<Vec<_>>();
        let mut result = CredentialMigrationResult {
            migrated: 0,
            warnings: Vec::new(),
        };
        for (id, name, token) in candidates {
            match self.backend.save_refresh_token(id, &token) {
                Ok(()) => {
                    if let Some(character) = locked
                        .store_mut()
                        .characters
                        .iter_mut()
                        .find(|character| character.id == id)
                    {
                        character.refresh_token = None;
                    }
                    // Persist each migration before touching another credential.
                    locked.persist()?;
                    result.migrated += 1;
                }
                Err(error) => result.warnings.push(format!(
                    "Could not move the refresh token for {name} to the system credential store: {error}"
                )),
            }
        }
        if result.migrated > 0 {
            self.changed();
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{test_support::*, timing::unix_time};
    use crate::models::ApiCooldown;
    use std::sync::Arc;

    #[test]
    fn reauthentication_during_cooldown_preserves_known_corporation_protection() {
        let backend = Arc::new(TestBackend::new());
        *backend
            .authenticated
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Character {
            id: 1,
            name: "Pilot renamed".into(),
            refresh_token: None,
            corporation_id: None,
            corporation_name: None,
        });
        let mut store = postable_store();
        store.characters[0].corporation_id = Some(100);
        store.characters[0].corporation_name = Some("Known Corp".into());
        store.api_cooldowns.push(ApiCooldown {
            source: "esi".into(),
            scope: None,
            until: unix_time() + 600,
            reason: None,
        });
        let core = Core::in_memory(backend, store);

        core.authenticate(
            &Cancellation::new(),
            AuthFlow::Loopback {
                open_browser: false,
            },
            &|_| {},
        )
        .unwrap();
        let character = core.snapshot().unwrap().store.characters.remove(0);

        assert_eq!(character.corporation_id, Some(100));
        assert_eq!(character.corporation_name.as_deref(), Some("Known Corp"));
    }
}
