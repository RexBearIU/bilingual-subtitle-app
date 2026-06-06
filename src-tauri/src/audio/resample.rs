//! Resampler: arbitrary-rate interleaved f32 → 16 kHz mono f32.
//! Wired into the pipeline in M3 (VAD/ASR).

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub struct Resampler16k {
    inner: SincFixedIn<f32>,
    channels: usize,
    out_bufs: Vec<Vec<f32>>,
    /// Leftover input that hasn't filled a full rubato chunk yet.
    pending: Vec<Vec<f32>>,
}

impl Resampler16k {
    pub fn new(input_rate: u32, channels: usize) -> Result<Self, String> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = 16_000.0 / input_rate as f64;
        let chunk_size = 1024usize;
        let inner = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk_size, channels)
            .map_err(|e| format!("rubato init: {e}"))?;
        let out_bufs = inner.output_buffer_allocate(true);
        let pending = vec![Vec::new(); channels];
        Ok(Self { inner, channels, out_bufs, pending })
    }

    /// Feed interleaved f32 samples; returns any available 16 kHz mono output.
    pub fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, String> {
        // De-interleave into per-channel pending buffers.
        for (ch, pend) in self.pending.iter_mut().enumerate() {
            pend.extend(interleaved.iter().skip(ch).step_by(self.channels).copied());
        }

        let chunk = self.inner.input_frames_next();
        let mut mono_out = Vec::new();

        while self.pending[0].len() >= chunk {
            let in_bufs: Vec<Vec<f32>> = self
                .pending
                .iter_mut()
                .map(|ch| ch.drain(..chunk).collect())
                .collect();

            let (_, out_len) = self
                .inner
                .process_into_buffer(&in_bufs, &mut self.out_bufs, None)
                .map_err(|e| format!("rubato process: {e}"))?;

            // Mix channels to mono.
            for i in 0..out_len {
                let sum: f32 = self.out_bufs.iter().map(|ch| ch[i]).sum();
                mono_out.push(sum / self.channels as f32);
            }
        }

        Ok(mono_out)
    }
}
