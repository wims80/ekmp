use crate::{integrations::images, persistence::image_cache};
use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub(super) use images::IdentityImageKey;

pub(super) struct DecodedIdentityImage {
    pub(super) key: IdentityImageKey,
    pub(super) size: [usize; 2],
    pub(super) rgba: Vec<u8>,
}

pub(super) enum IdentityImageEvent {
    Loaded(DecodedIdentityImage),
    Failed(IdentityImageKey),
}

pub(super) fn start_identity_image_worker(
    load_images: bool,
) -> (Sender<IdentityImageKey>, Receiver<IdentityImageEvent>) {
    let (request_tx, request_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        for key in request_rx {
            let event = if load_images {
                match load_identity_image(key) {
                    Ok(image) => IdentityImageEvent::Loaded(image),
                    Err(()) => IdentityImageEvent::Failed(key),
                }
            } else {
                IdentityImageEvent::Failed(key)
            };
            if event_tx.send(event).is_err() {
                break;
            }
        }
    });
    (request_tx, event_rx)
}

fn load_identity_image(key: IdentityImageKey) -> Result<DecodedIdentityImage, ()> {
    let cached = image_cache::load(key).ok().flatten();
    if let Some(cached) = &cached {
        if cached.fresh {
            if let Ok(image) = decode_identity_image(key, &cached.bytes) {
                return Ok(image);
            }
        }
    }

    match images::fetch(key) {
        Ok(bytes) => {
            let image = decode_identity_image(key, &bytes)?;
            let _ = image_cache::store(key, &bytes);
            Ok(image)
        }
        Err(_) => cached
            .and_then(|cached| decode_identity_image(key, &cached.bytes).ok())
            .ok_or(()),
    }
}

fn decode_identity_image(key: IdentityImageKey, bytes: &[u8]) -> Result<DecodedIdentityImage, ()> {
    let decoded = image::load_from_memory(bytes).map_err(|_| ())?.into_rgba8();
    let size = [decoded.width() as usize, decoded.height() as usize];
    Ok(DecodedIdentityImage {
        key,
        size,
        rgba: decoded.into_raw(),
    })
}
