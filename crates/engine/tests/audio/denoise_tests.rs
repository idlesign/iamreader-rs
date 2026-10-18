use super::stft_istft_roundtrip;

fn waveform(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| {
            let time = index as f32 / 44_100.0;
            0.45 * (std::f32::consts::TAU * 317.0 * time).sin()
                + 0.2 * (std::f32::consts::TAU * 2_113.0 * time).cos()
        })
        .collect()
}

fn assert_roundtrip_close(input: &[f32]) {
    let original = input.to_vec();
    let reconstructed = stft_istft_roundtrip(input).unwrap();
    assert_eq!(reconstructed.len(), input.len());
    assert_eq!(input, original, "Roundtrip must not change its input");
    for (index, (&actual, &expected)) in reconstructed.iter().zip(input).enumerate() {
        assert!(actual.is_finite(), "Non-finite output at sample {index}");
        assert!(
            (actual - expected).abs() <= 1e-5,
            "Roundtrip error at sample {index}: expected {expected}, got {actual}"
        );
    }
}

#[test]
fn roundtrip_preserves_empty_and_short_signals_at_hop_and_fft_boundaries() {
    assert_eq!(stft_istft_roundtrip(&[]).unwrap(), Vec::<f32>::new());
    for length in [1, 2, 419, 420, 421, 1_679, 1_680, 1_681, 5_041] {
        assert_roundtrip_close(&waveform(length));
    }
}

#[test]
fn roundtrip_is_deterministic_across_multiple_frames_and_repeated_calls() {
    let input = waveform(6_731);
    let first = stft_istft_roundtrip(&input).unwrap();
    let different: Vec<_> = input.iter().map(|sample| -*sample * 0.3).collect();
    assert_roundtrip_close(&different);
    let second = stft_istft_roundtrip(&input).unwrap();
    let third = stft_istft_roundtrip(&input).unwrap();
    assert_eq!(first, second);
    assert_eq!(second, third);
    assert_roundtrip_close(&input);
}

#[test]
fn reused_fft_buffers_preserve_silence_constant_levels_and_impulses() {
    let silence = vec![0.0; 5_041];
    assert_eq!(stft_istft_roundtrip(&silence).unwrap(), silence);
    for level in [-0.75, 0.25, 1.0] {
        assert_roundtrip_close(&vec![level; 1_681]);
    }
    let mut impulses = vec![0.0; 5_041];
    for (index, value) in [
        (0, 1.0),
        (419, -0.5),
        (420, 0.25),
        (1_681, -0.75),
        (5_040, 1.0),
    ] {
        impulses[index] = value;
    }
    assert_roundtrip_close(&impulses);
}
