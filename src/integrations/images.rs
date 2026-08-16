use reqwest::{blocking::Client, header::USER_AGENT};

const IMAGE_SERVICE: &str = "https://images.evetech.net";
const USER_AGENT_VALUE: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/wims80/ekmp)"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IdentityImageKey {
    Character(u64),
    Corporation(u64),
}

impl IdentityImageKey {
    fn url(self) -> String {
        match self {
            Self::Character(id) => {
                format!("{IMAGE_SERVICE}/characters/{id}/portrait?size=128")
            }
            Self::Corporation(id) => {
                format!("{IMAGE_SERVICE}/corporations/{id}/logo?size=128")
            }
        }
    }

    pub(crate) fn texture_name(self) -> String {
        match self {
            Self::Character(id) => format!("character-portrait-{id}"),
            Self::Corporation(id) => format!("corporation-logo-{id}"),
        }
    }

    pub(crate) fn cache_file_name(self) -> String {
        match self {
            Self::Character(id) => format!("character-{id}.jpg"),
            Self::Corporation(id) => format!("corporation-{id}.png"),
        }
    }
}

pub(crate) fn fetch(key: IdentityImageKey) -> Result<Vec<u8>, String> {
    Client::new()
        .get(key.url())
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .map_err(|error| format!("EVE image request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("EVE image request failed: {error}"))?
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("EVE image response could not be read: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_image_urls_use_the_official_service_and_supported_size() {
        assert_eq!(
            IdentityImageKey::Character(42).url(),
            "https://images.evetech.net/characters/42/portrait?size=128"
        );
        assert_eq!(
            IdentityImageKey::Corporation(84).url(),
            "https://images.evetech.net/corporations/84/logo?size=128"
        );
    }

    #[test]
    fn identity_images_have_format_specific_cache_names() {
        assert_eq!(
            IdentityImageKey::Character(42).cache_file_name(),
            "character-42.jpg"
        );
        assert_eq!(
            IdentityImageKey::Corporation(84).cache_file_name(),
            "corporation-84.png"
        );
    }
}
