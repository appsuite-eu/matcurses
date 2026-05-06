//! Event sound assets (OGG/Vorbis), borrowed from element-web's
//! `apps/web/res/media/`. Used for non-blocking notifications:
//! incoming message, mention, call ringing, call ended, error.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SoundKind {
    Message,
    Mention,
    Ring,
    Ringback,
    CallEnd,
    Busy,
    Error,
}

const MESSAGE_OGG: &[u8] = include_bytes!("../assets/sounds/message.ogg");
const RING_OGG: &[u8] = include_bytes!("../assets/sounds/ring.ogg");
const RINGBACK_OGG: &[u8] = include_bytes!("../assets/sounds/ringback.ogg");
const CALLEND_OGG: &[u8] = include_bytes!("../assets/sounds/callend.ogg");
const BUSY_OGG: &[u8] = include_bytes!("../assets/sounds/busy.ogg");
const ERROR_OGG: &[u8] = include_bytes!("../assets/sounds/error.ogg");

pub fn bytes_for(kind: SoundKind) -> &'static [u8] {
    match kind {
        SoundKind::Message => MESSAGE_OGG,
        // No dedicated mention sound from Element; use the alert tone so
        // mentions stand out against regular incoming messages.
        SoundKind::Mention => ERROR_OGG,
        SoundKind::Ring => RING_OGG,
        SoundKind::Ringback => RINGBACK_OGG,
        SoundKind::CallEnd => CALLEND_OGG,
        SoundKind::Busy => BUSY_OGG,
        SoundKind::Error => ERROR_OGG,
    }
}
