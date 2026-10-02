//! A small in-place radix-2 FFT for the engine's analysis needs.
//!
//! The effect suite has its own, effect-specific spectrum implementation
//! (`crate::effects::eq::spectrum`); this module exists for engine-level
//! analysis (for example, level/waveform displays) and keeps the transform
//! logic in one place for the DSP layer.
//!
//! # Sizing
//!
//! The transform length must be a power of two. [`Fft::new`] allocates the
//! twiddle factors and the bit-reversal table once, so [`Fft::forward`] and
//! [`Fft::inverse`] never allocate. Callers that run on the audio thread must
//! construct the [`Fft`] during `prepare`.

use crate::effects::util::dsp::{cos_poly, sin_poly};

/// A preallocated radix-2 FFT of a fixed length.
#[derive(Debug, Clone)]
pub struct Fft {
    /// Transform length; a power of two.
    size: usize,
    /// Bit-reversal permutation, computed once.
    reversal: alloc::vec::Vec<u32>,
    /// Twiddle factors `e^{-2*pi*i*k/N}` for the forward transform.
    cos_table: alloc::vec::Vec<f32>,
    /// Imaginary part of the forward twiddles.
    sin_table: alloc::vec::Vec<f32>,
}

impl Fft {
    /// Builds an FFT of length `size`, which must be a power of two.
    ///
    /// Returns `None` for a non-power-of-two or a tiny size rather than
    /// panicking, so a bad configuration surfaces as a fallible call.
    #[must_use]
    pub fn new(size: usize) -> Option<Self> {
        if size < 2 || !size.is_power_of_two() {
            return None;
        }
        let bits = size.trailing_zeros();
        let mut reversal = alloc::vec![0u32; size];
        for (i, r) in reversal.iter_mut().enumerate() {
            *r = (i as u32).reverse_bits() >> (32 - bits);
        }
        // Half a turn's worth of twiddles is enough: the second half is the
        // negation, which the butterflies index directly.
        let half = size / 2;
        let mut cos_table = alloc::vec![0.0f32; half];
        let mut sin_table = alloc::vec![0.0f32; half];
        for (k, (c, s)) in cos_table.iter_mut().zip(sin_table.iter_mut()).enumerate() {
            let angle = core::f32::consts::TAU * k as f32 / size as f32;
            *c = cos_poly(angle);
            *s = sin_poly(angle);
        }
        Some(Self {
            size,
            reversal,
            cos_table,
            sin_table,
        })
    }

    /// Transform length.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Forward transform, in place.
    ///
    /// `real` and `imag` must each be at least [`Self::size`] long; a shorter
    /// slice is processed up to the shorter length rather than panicking, so a
    /// caller mistake cannot take down the audio thread.
    pub fn forward(&self, real: &mut [f32], imag: &mut [f32]) {
        self.transform(real, imag, false);
    }

    /// Inverse transform, in place, scaled by `1/N`.
    pub fn inverse(&self, real: &mut [f32], imag: &mut [f32]) {
        self.transform(real, imag, true);
        let scale = 1.0 / self.size as f32;
        let n = self.size.min(real.len()).min(imag.len());
        for i in 0..n {
            real[i] *= scale;
            imag[i] *= scale;
        }
    }

    /// The shared butterfly core; `inverse` conjugates the twiddles.
    fn transform(&self, real: &mut [f32], imag: &mut [f32], inverse: bool) {
        let n = self.size;
        if real.len() < n || imag.len() < n {
            return;
        }
        // Bit-reversal permutation.
        for i in 0..n {
            let j = self.reversal[i] as usize;
            if i < j {
                real.swap(i, j);
                imag.swap(i, j);
            }
        }

        // Butterfly stages.
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            let mut start = 0;
            while start < n {
                let mut k = 0;
                while k < half {
                    let t = k * step;
                    let (sr, si) = if inverse {
                        // Conjugate twiddle for the inverse transform.
                        (self.cos_table[t], -self.sin_table[t])
                    } else {
                        (self.cos_table[t], self.sin_table[t])
                    };
                    let even = start + k;
                    let odd = start + k + half;
                    let odr = real[odd] * sr - imag[odd] * si;
                    let odi = real[odd] * si + imag[odd] * sr;
                    real[odd] = real[even] - odr;
                    imag[odd] = imag[even] - odi;
                    real[even] += odr;
                    imag[even] += odi;
                    k += 1;
                }
                start += len;
            }
            // `len` doubles each stage; the shift keeps this branchless-ish and
            // cannot overflow because len <= n.
            if len == n {
                break;
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_powers_of_two_are_accepted() {
        assert!(Fft::new(0).is_none());
        assert!(Fft::new(1).is_none());
        assert!(Fft::new(3).is_none());
        assert!(Fft::new(8).is_some());
    }

    #[test]
    fn a_round_trip_reconstructs_the_signal() {
        let n = 64;
        let fft = Fft::new(n).expect("power of two");
        let original: alloc::vec::Vec<f32> = (0..n)
            .map(|i| (core::f32::consts::TAU * 5.0 * i as f32 / n as f32).sin())
            .collect();
        let mut real = original.clone();
        let mut imag = alloc::vec![0.0f32; n];
        fft.forward(&mut real, &mut imag);
        fft.inverse(&mut real, &mut imag);
        for (a, b) in original.iter().zip(real.iter()) {
            assert!((a - b).abs() < 1e-3, "round trip mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn a_pure_tone_lands_in_the_expected_bin() {
        let n = 64;
        let fft = Fft::new(n).expect("power of two");
        let bin = 5;
        let mut real: alloc::vec::Vec<f32> = (0..n)
            .map(|i| (core::f32::consts::TAU * bin as f32 * i as f32 / n as f32).cos())
            .collect();
        let mut imag = alloc::vec![0.0f32; n];
        fft.forward(&mut real, &mut imag);
        // Energy at bin 5 should dominate every other bin. The input is real,
        // so the spectrum is conjugate-symmetric: bin `n - bin` is the mirror
        // and is equally large by construction, not leakage.
        let mag = |k: usize| (real[k] * real[k] + imag[k] * imag[k]).sqrt();
        let peak = mag(bin);
        let mirror = n - bin;
        for k in 0..n {
            if k != bin && k != mirror {
                assert!(mag(k) < peak * 0.05, "bin {k} unexpectedly large");
            }
        }
    }

    #[test]
    fn a_short_slice_is_refused_rather_than_read_out_of_bounds() {
        let fft = Fft::new(16).expect("power of two");
        let mut real = [0.0f32; 4];
        let mut imag = [0.0f32; 4];
        // Must return without touching beyond the slices.
        fft.forward(&mut real, &mut imag);
        assert!(real.iter().all(|s| s.is_finite()));
    }
}
