/// RMS (root mean square) of a f32 sample slice. Returns 0.0 for empty input.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Linear RMS → dBFS, clamped to –90 dBFS floor.
pub fn rms_to_dbfs(rms: f32) -> f32 {
    if rms <= 1e-9 {
        return -90.0;
    }
    (20.0 * rms.log10()).max(-90.0)
}
