use rox_playback::chain::{Chain, Node};
use rox_playback::convolver::{Convolver, ConvolverMode, ConvolverParams, IrLayout, parse_wav};
use std::sync::Arc;

const SAMPLE_RATE: u32 = 48000;

/// Create a synthetic 14-channel HeSuVi HRIR WAV in memory.
/// Channel mapping:
/// 0: FL->L, 1: FL->R
/// 2: SL->L, 3: SL->R
/// 4: BL->L, 5: BL->R
/// 6: FC->L
/// 7: FR->R, 8: FR->L (right ear first!)
/// 9: SR->R, 10: SR->L
/// 11: BR->R, 12: BR->L
/// 13: FC->R
fn generate_synthetic_hesuvi_wav(rate: u32, len: usize) -> Vec<u8> {
    let channels = 14u16;
    let mut buf = Vec::new();
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&rate.to_le_bytes());
    let block_align = channels * 2;
    let byte_rate = rate * (block_align as u32);
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes()); // 16-bit

    buf.extend_from_slice(b"data");
    let data_len = (len * block_align as usize) as u32;
    buf.extend_from_slice(&data_len.to_le_bytes());

    for i in 0..len {
        for ch in 0..channels {
            // Realistic delays:
            // Direct ear: peak around sample 5
            // Contralateral ear: peak around sample 18 (~270 us head delay)
            let is_contra = matches!(ch, 1 | 8 | 3 | 10 | 5 | 12);
            let peak_pos = if is_contra { 18 } else { 5 };
            let val = if i == peak_pos {
                if is_contra { 0.4f32 } else { 0.8f32 }
            } else if i > peak_pos && i < peak_pos + 10 {
                // Pinna reflection
                0.15f32 * (-((i - peak_pos) as f32) / 3.0).exp()
            } else {
                0.0f32
            };
            let sample = (val * 32767.0) as i16;
            buf.extend_from_slice(&sample.to_le_bytes());
        }
    }

    let total = (buf.len() - 8) as u32;
    buf[4..8].copy_from_slice(&total.to_le_bytes());
    buf
}

#[test]
fn test_hesuvi_virtual_stereo_binaural_rendering() {
    let wav_data = generate_synthetic_hesuvi_wav(SAMPLE_RATE, 64);
    let ir = parse_wav("hesuvi_test.wav", &wav_data).expect("Failed to parse HeSuVi WAV");
    assert_eq!(ir.layout, IrLayout::Hesuvi14);
    assert_eq!(ir.channels.len(), 14);

    let params = Arc::new(ConvolverParams::new(
        true,
        1.0, // 100% wet
        0.0,
        ConvolverMode::VirtualStereo,
        1.0, // Normal width
        0.0, // IR active
        None,
        Some(ir),
    ));

    let mut chain = Chain::new();
    chain.push(Box::new(Convolver::new(params.clone())));
    chain.reset(SAMPLE_RATE);

    // Feed an impulse only into the Left channel: [1.0, 0.0, 0.0, 0.0, ...]
    let frames = 64;
    let mut buf = vec![0.0f32; frames * 2];
    buf[0] = 1.0;
    buf[1] = 0.0;

    chain.process(&mut buf);

    // Left channel should have direct ear peak around frame 5
    let left_samples: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let right_samples: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();

    let left_max_idx = left_samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    let right_max_idx = right_samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;

    // Direct ear peak should arrive earlier than contralateral ear peak!
    assert_eq!(
        left_max_idx, 5,
        "Left ear must receive direct sound at sample 5"
    );
    assert_eq!(
        right_max_idx, 18,
        "Right ear must receive cross-ear sound at sample 18"
    );
    assert!(
        left_samples[left_max_idx] > right_samples[right_max_idx],
        "Direct ear must be louder than contralateral ear (ILD cue)"
    );
}

#[test]
fn test_hesuvi_surround_upmix_rendering() {
    let wav_data = generate_synthetic_hesuvi_wav(SAMPLE_RATE, 64);
    let ir = parse_wav("hesuvi_test.wav", &wav_data).unwrap();

    let params = Arc::new(ConvolverParams::new(
        true,
        1.0,
        0.0,
        ConvolverMode::Surround7_1,
        1.0,
        0.0,
        None,
        Some(ir),
    ));

    let mut convolver = Convolver::new(params.clone());
    convolver.reset(SAMPLE_RATE);

    // Feed stereo audio
    let mut buf = vec![0.0f32; 128];
    for i in (0..buf.len()).step_by(2) {
        buf[i] = 0.7;
        buf[i + 1] = 0.3;
    }

    convolver.process(&mut buf);

    // Check that output contains valid non-zero finite audio
    for &sample in &buf {
        assert!(sample.is_finite());
    }
    let energy: f32 = buf.iter().map(|s| s.powi(2)).sum();
    assert!(energy > 0.01, "Surround upmix must produce energy");
}

#[test]
fn test_live_switching_of_modes_and_ir() {
    let wav_data = generate_synthetic_hesuvi_wav(SAMPLE_RATE, 32);
    let ir = parse_wav("hesuvi_test.wav", &wav_data).unwrap();

    let params = Arc::new(ConvolverParams::new(
        true,
        1.0,
        0.0,
        ConvolverMode::VirtualStereo,
        1.0,
        0.0,
        None,
        None, // start with built-in spatial
    ));

    let mut convolver = Convolver::new(params.clone());
    convolver.reset(SAMPLE_RATE);

    let mut buf = vec![0.5f32; 64];
    convolver.process(&mut buf);
    assert!(buf[0].is_finite());

    // Switch IR live mid-stream
    params.set_ir(Some(ir));
    convolver.process(&mut buf);
    assert!(buf[0].is_finite());

    // Switch mode live
    params.set_mode(ConvolverMode::Surround7_1);
    convolver.process(&mut buf);
    assert!(buf[0].is_finite());

    // Switch wet mix live
    params.set_wet(0.5);
    convolver.process(&mut buf);
    assert!(buf[0].is_finite());

    // Disable live -> bypass
    params.set_enabled(false);
    let dry = vec![0.333f32; 64];
    let mut test_buf = dry.clone();
    convolver.process(&mut test_buf);
    assert_eq!(
        test_buf, dry,
        "Disabled convolver must be bit-exact passthrough"
    );
}

#[test]
fn test_real_atmos_hesuvi_wav_loading_and_convolving() {
    use rox_playback::convolver::BuiltinHesuviProfile;

    let ir = BuiltinHesuviProfile::Atmos
        .load_ir()
        .expect("Atmos profile must be bundled");
    assert_eq!(ir.layout, IrLayout::Hesuvi14);
    assert_eq!(ir.channels.len(), 14);
    assert_eq!(ir.sample_rate, 48000);

    let params = Arc::new(ConvolverParams::new(
        true,
        1.0,
        0.0,
        ConvolverMode::VirtualStereo,
        1.0,
        0.0,
        None,
        Some(ir),
    ));
    let mut convolver = Convolver::new(params);
    convolver.reset(48000);

    let mut buf = vec![0.5f32; 256];
    convolver.process(&mut buf);
    for &sample in &buf {
        assert!(sample.is_finite());
    }
}

#[test]
fn test_realtime_performance_benchmark() {
    use rox_playback::convolver::BuiltinHesuviProfile;

    let ir = BuiltinHesuviProfile::Atmos
        .load_ir()
        .expect("Atmos profile must be bundled");

    let params = Arc::new(ConvolverParams::new(
        true,
        1.0,
        0.0,
        ConvolverMode::Surround7_1, // Max load: full 14 channels!
        1.0,
        0.0,
        None,
        Some(ir),
    ));
    let mut convolver = Convolver::new(params);
    convolver.reset(48000);

    // 1 full second of stereo audio = 48,000 frames = 96,000 samples
    let mut one_second_audio = vec![0.3f32; 96000];

    let start = std::time::Instant::now();
    convolver.process(&mut one_second_audio);
    let elapsed = start.elapsed();

    eprintln!(
        "Surround 7.1 (14 channels) processed 1.0 second of audio in {:?}",
        elapsed
    );

    for &sample in &one_second_audio {
        assert!(sample.is_finite());
    }
}
