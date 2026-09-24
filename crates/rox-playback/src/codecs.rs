//! The one codec registry every decode goes through: symphonia's enabled
//! codecs plus the Opus decoder in [`crate::opus`]. Playback, ReplayGain
//! measurement, and the acoustic extractor all share it so they can never
//! disagree about what decodes.

use std::sync::OnceLock;

use symphonia::core::codecs::registry::CodecRegistry;

pub fn registry() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut reg = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut reg);
        reg.register_audio_decoder::<crate::opus::OpusDecoder>();
        reg
    })
}

#[cfg(test)]
mod tests {
    use symphonia::core::codecs::audio::well_known::{CODEC_ID_FLAC, CODEC_ID_OPUS};

    #[test]
    fn the_registry_holds_opus_beside_the_built_in_codecs() {
        let reg = super::registry();
        assert!(
            reg.get_audio_decoder(CODEC_ID_OPUS).is_some(),
            "the whole reason this registry exists"
        );
        assert!(
            reg.get_audio_decoder(CODEC_ID_FLAC).is_some(),
            "and symphonia's own codecs are still there"
        );
    }
}
