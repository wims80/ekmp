use super::{timing::active_api_cooldown, Core, CoreError, CoreResult};
use crate::models::{ProtectedVictim, ProtectedVictimKind, Store};

impl Core {
    pub(crate) fn set_show_protected(&self, show: bool) -> CoreResult<()> {
        self.simple_mutation(|store| store.show_protected_killmails = show)
    }

    pub(crate) fn set_killmail_protection(&self, id: u64, protected: bool) -> CoreResult<()> {
        let mut locked = self.begin_operation()?;
        if !locked
            .store()
            .cached_killmails
            .iter()
            .any(|mail| mail.id == id)
        {
            return Err(CoreError::Operational(format!(
                "killmail {id} is not in the cache"
            )));
        }
        let ids = &mut locked.store_mut().manually_protected_killmail_ids;
        ids.retain(|old| *old != id);
        if protected {
            ids.push(id);
        }
        locked.persist()?;
        self.changed();
        Ok(())
    }

    pub(crate) fn add_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> CoreResult<ProtectedVictim> {
        let mut locked = self.begin_operation()?;
        if let Some(until) = active_api_cooldown(locked.store(), "esi") {
            return Err(CoreError::Operational(format!(
                "ESI cooldown is active until {until}"
            )));
        }
        let resolved = self.backend.resolve_protected_victim(kind, query);
        self.absorb_api_observations(locked.store_mut());
        locked.persist()?;
        let victim = resolved.map_err(|error| {
            CoreError::Operational(format!("could not resolve victim: {error}"))
        })?;
        let already_present = protected_victim_present(locked.store(), kind, victim.id);
        if !already_present {
            match kind {
                ProtectedVictimKind::Character => locked
                    .store_mut()
                    .manually_protected_characters
                    .push(victim.clone()),
                ProtectedVictimKind::Corporation => locked
                    .store_mut()
                    .manually_protected_corporations
                    .push(victim.clone()),
            }
            locked.persist()?;
            self.changed();
        }
        Ok(victim)
    }

    pub(crate) fn remove_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        id: u64,
    ) -> CoreResult<bool> {
        let mut locked = self.begin_operation()?;
        let entries = match kind {
            ProtectedVictimKind::Character => &mut locked.store_mut().manually_protected_characters,
            ProtectedVictimKind::Corporation => {
                &mut locked.store_mut().manually_protected_corporations
            }
        };
        let old_len = entries.len();
        entries.retain(|entry| entry.id != id);
        let removed = entries.len() != old_len;
        if removed {
            locked.persist()?;
            self.changed();
        }
        Ok(removed)
    }
}
fn protected_victim_present(store: &Store, kind: ProtectedVictimKind, id: u64) -> bool {
    match kind {
        ProtectedVictimKind::Character => {
            store.characters.iter().any(|entry| entry.id == id)
                || store
                    .manually_protected_characters
                    .iter()
                    .any(|entry| entry.id == id)
        }
        ProtectedVictimKind::Corporation => {
            store
                .characters
                .iter()
                .any(|entry| entry.corporation_id == Some(id))
                || store
                    .manually_protected_corporations
                    .iter()
                    .any(|entry| entry.id == id)
        }
    }
}
