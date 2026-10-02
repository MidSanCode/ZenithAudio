//! cfg-dispatched SIMD kernels for the effect suite's hot loops (PLAN section 3.S5).
//!
//! # Why hand-written rather than `std::simd`
//!
//! The plan names `std::simd` *or* platform intrinsics and leaves the choice to
//! the implementer. `std::simd` is still unstable (`portable_simd`, rust issue
//! 86656), and this crate is not on nightly, so the stable option is the
//! target-specific `core::arch` intrinsics behind `cfg`. That is also the form
//! the plan describes for the web target: `wasm32` uses `simd128`.
//!
//! # Dispatch
//!
//! Each public kernel compiles exactly one implementation:
//!
//! * `aarch64` - NEON. On `aarch64` NEON is part of the baseline, so the
//!   intrinsics are available without a `target_feature` gate (the target's own
//!   `cfg` reports `target_feature="neon"`).
//! * `wasm32` - `simd128`, behind `#[target_feature(enable = "simd128")]` so
//!   the SIMD code is emitted even when the crate is not built with a global
//!   `+simd128` flag. This is deliberate: the web build must exercise SIMD, and
//!   requiring every caller to remember a rustflag is how a "SIMD path" quietly
//!   becomes dead code. The consequence, stated so it is not a surprise, is that
//!   a `wasm32` module built this way needs an engine with SIMD support, which
//!   every current browser and Node release has.
//! * everything else - a scalar fallback, so the crate keeps building for any
//!   target the rest of it supports.
//!
//! # Correctness
//!
//! SIMD changes the *order* of a sum, so results are equal in the mathematical
//! sense but not always bit-for-bit to the scalar loop. Every kernel below is
//! therefore tested for numerical closeness against a scalar reference, not for
//! exact equality, and every effect's own tests already allow the tiny
//! reassociation error a fused multiply-add introduces.

/// The number of `f32` lanes each SIMD path processes at once.
///
/// Only the `aarch64` and `wasm32` kernels use it; on the scalar-only targets
/// (notably the x86_64 CI runner) there is no SIMD path to size, and leaving it
/// ungated made it dead code there — which `cargo clippy -D warnings` (the CI
/// gate) rejects. Gating it to the architectures that consume it keeps the
/// scalar build warning-free without an `#[allow]` that would hide a real
/// regression.
#[cfg(any(target_arch = "aarch64", target_arch = "wasm32"))]
const LANES: usize = 4;

/// Multiplies every element of `samples` by `gain`.
///
/// The saturation and multimode drive stages both end with this exact loop, and
/// the plan names saturation as one of its hotspots.
#[inline]
pub fn scale_in_place(samples: &mut [f32], gain: f32) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is baseline on `aarch64`; the kernel reads and writes
        // only within `samples`.
        unsafe { aarch64::scale_in_place(samples, gain) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: guarded by the `simd128` target feature. The kernel reads and
        // writes only within `samples`.
        unsafe { wasm::scale_in_place(samples, gain) };
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "wasm32")))]
    {
        scalar::scale_in_place(samples, gain);
    }
}

/// Computes `out[i] = input[i] * mul + add` for each element.
///
/// This is the saturator's "apply drive and bias" loop, which runs before the
/// waveshaper on every sample of every block.
///
/// # Panics
///
/// Panics if `out.len() != input.len()`, which is a programming error rather
/// than a runtime condition: the caller owns both buffers.
#[inline]
pub fn affine_in_place(out: &mut [f32], input: &[f32], mul: f32, add: f32) {
    assert_eq!(out.len(), input.len(), "affine_in_place length mismatch");
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: same argument as `scale_in_place`; both slices are equal
        // length by the assertion above.
        unsafe { aarch64::affine_in_place(out, input, mul, add) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: same argument as `scale_in_place`.
        unsafe { wasm::affine_in_place(out, input, mul, add) };
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "wasm32")))]
    {
        scalar::affine_in_place(out, input, mul, add);
    }
}

/// Accumulates the complex product of two spectra into `accum`.
///
/// For every bin:
///
/// ```text
///   accum_re[i] += h_re[i] * ir_re[i] - h_im[i] * ir_im[i]
///   accum_im[i] += h_re[i] * ir_im[i] + h_im[i] * ir_re[i]
/// ```
///
/// This is the inner loop of the partitioned convolution reverb: it runs once
/// per partition per block over the full `FFT_SIZE`, which makes it the single
/// hottest loop in the suite.
///
/// The four source slices must be the same length; the accumulators may be
/// longer, because the caller accumulates into a full-spectrum buffer while
/// reading one partition. Only the first `h_re.len()` bins are touched.
///
/// # Panics
///
/// Panics if the four source slices or the two accumulators disagree in length,
/// which would otherwise read out of bounds on a later iteration.
#[inline]
pub fn complex_mac(
    accum_re: &mut [f32],
    accum_im: &mut [f32],
    h_re: &[f32],
    h_im: &[f32],
    ir_re: &[f32],
    ir_im: &[f32],
) {
    let n = h_re.len();
    assert_eq!(h_im.len(), n, "complex_mac imaginary history mismatch");
    assert_eq!(ir_re.len(), n, "complex_mac real kernel mismatch");
    assert_eq!(ir_im.len(), n, "complex_mac imaginary kernel mismatch");
    assert!(accum_re.len() >= n, "complex_mac real accumulator too short");
    assert!(accum_im.len() >= n, "complex_mac imaginary accumulator too short");
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: all six slices are at least `n` long, checked above.
        unsafe { aarch64::complex_mac(accum_re, accum_im, h_re, h_im, ir_re, ir_im) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: same length argument as `scale_in_place`.
        unsafe { wasm::complex_mac(accum_re, accum_im, h_re, h_im, ir_re, ir_im) };
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "wasm32")))]
    {
        scalar::complex_mac(accum_re, accum_im, h_re, h_im, ir_re, ir_im);
    }
}

/// Multiplies two equal-length slices element-wise into `out`.
///
/// `out[i] = a[i] * b[i]`. This is the EQ spectrum analyser's windowing loop:
/// it multiplies the input ring by the precomputed Hann window once per
/// analysis frame. The caller handles the ring's wrap by calling this twice on
/// the two contiguous runs, which is also why this kernel takes a plain
/// `out`/`a`/`b` and not the ring itself.
///
/// # Panics
///
/// Panics if the three slices disagree in length.
#[inline]
pub fn mul_into(out: &mut [f32], a: &[f32], b: &[f32]) {
    assert_eq!(a.len(), b.len(), "mul_into operand length mismatch");
    assert_eq!(out.len(), a.len(), "mul_into output length mismatch");
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: all three slices are equal length, checked above.
        unsafe { aarch64::mul_into(out, a, b) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: all three slices are equal length, checked above.
        unsafe { wasm::mul_into(out, a, b) };
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "wasm32")))]
    {
        scalar::mul_into(out, a, b);
    }
}

/// The portable implementation, used on every target without a SIMD kernel.
///
/// Dead code on `aarch64` and `wasm32`, where the SIMD paths are selected at
/// compile time - but it is the *reference* the SIMD tests compare against, so
/// it must still compile and stay correct on every target.
#[allow(dead_code)]
mod scalar {
    /// Multiplies every element by `gain`.
    pub fn scale_in_place(samples: &mut [f32], gain: f32) {
        for sample in samples {
            *sample *= gain;
        }
    }

    /// `out = a * b`.
    pub fn mul_into(out: &mut [f32], a: &[f32], b: &[f32]) {
        for ((out, a), b) in out.iter_mut().zip(a).zip(b) {
            *out = *a * *b;
        }
    }

    /// `out = input * mul + add`.
    pub fn affine_in_place(out: &mut [f32], input: &[f32], mul: f32, add: f32) {
        for (out, input) in out.iter_mut().zip(input) {
            *out = *input * mul + add;
        }
    }

    /// Complex multiply-accumulate over the first `h_re.len()` bins.
    pub fn complex_mac(
        accum_re: &mut [f32],
        accum_im: &mut [f32],
        h_re: &[f32],
        h_im: &[f32],
        ir_re: &[f32],
        ir_im: &[f32],
    ) {
        for i in 0..h_re.len() {
            let hr = h_re[i];
            let hi = h_im[i];
            let irr = ir_re[i];
            let iri = ir_im[i];
            accum_re[i] += hr * irr - hi * iri;
            accum_im[i] += hr * iri + hi * irr;
        }
    }
}

/// NEON kernels, used on every `aarch64`.
///
/// NEON is part of the `aarch64` baseline, so these need no `target_feature`
/// attribute; they are `unsafe` only because the raw intrinsics are.
#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::aarch64::*;

    /// Multiplies every element by `gain`.
    ///
    /// # Safety
    ///
    /// Reads and writes only within `samples`.
    pub unsafe fn scale_in_place(samples: &mut [f32], gain: f32) {
        let n = samples.len();
        let vector_gain = vdupq_n_f32(gain);
        let mut i = 0;
        // SAFETY: `i + LANES <= n` on every load/store below.
        unsafe {
            while i + super::LANES <= n {
                let v = vld1q_f32(samples.as_ptr().add(i));
                vst1q_f32(samples.as_mut_ptr().add(i), vmulq_f32(v, vector_gain));
                i += super::LANES;
            }
        }
        while i < n {
            samples[i] *= gain;
            i += 1;
        }
    }

    /// `out = a * b`.
    ///
    /// # Safety
    ///
    /// All three slices must be the same length.
    pub unsafe fn mul_into(out: &mut [f32], a: &[f32], b: &[f32]) {
        let n = out.len();
        let mut i = 0;
        // SAFETY: all three slices are equal length and `i + LANES <= n`.
        unsafe {
            while i + super::LANES <= n {
                let va = vld1q_f32(a.as_ptr().add(i));
                let vb = vld1q_f32(b.as_ptr().add(i));
                vst1q_f32(out.as_mut_ptr().add(i), vmulq_f32(va, vb));
                i += super::LANES;
            }
        }
        while i < n {
            out[i] = a[i] * b[i];
            i += 1;
        }
    }

    /// `out = input * mul + add`.
    ///
    /// # Safety
    ///
    /// `out` and `input` must be the same length; each is read/written only
    /// within its own bounds.
    pub unsafe fn affine_in_place(out: &mut [f32], input: &[f32], mul: f32, add: f32) {
        let n = out.len();
        let vector_mul = vdupq_n_f32(mul);
        let vector_add = vdupq_n_f32(add);
        let mut i = 0;
        // SAFETY: `out` and `input` are equal length (asserted by the caller)
        // and `i + LANES <= n` on every access below.
        unsafe {
            while i + super::LANES <= n {
                let v = vld1q_f32(input.as_ptr().add(i));
                let r = vfmaq_f32(vector_add, v, vector_mul);
                vst1q_f32(out.as_mut_ptr().add(i), r);
                i += super::LANES;
            }
        }
        while i < n {
            out[i] = input[i] * mul + add;
            i += 1;
        }
    }

    /// Complex multiply-accumulate.
    ///
    /// # Safety
    ///
    /// Every slice must be at least `h_re.len()` long.
    pub unsafe fn complex_mac(
        accum_re: &mut [f32],
        accum_im: &mut [f32],
        h_re: &[f32],
        h_im: &[f32],
        ir_re: &[f32],
        ir_im: &[f32],
    ) {
        let n = h_re.len();
        let mut i = 0;
        // SAFETY: all six slices are at least `n` long; `i + LANES <= n` holds
        // on every iteration.
        unsafe {
            while i + super::LANES <= n {
                let hr = vld1q_f32(h_re.as_ptr().add(i));
                let hi = vld1q_f32(h_im.as_ptr().add(i));
                let irr = vld1q_f32(ir_re.as_ptr().add(i));
                let iri = vld1q_f32(ir_im.as_ptr().add(i));
                // re += hr*irr - hi*iri ; im += hr*iri + hi*irr
                let re_inc = vsubq_f32(vmulq_f32(hr, irr), vmulq_f32(hi, iri));
                let im_inc = vaddq_f32(vmulq_f32(hr, iri), vmulq_f32(hi, irr));
                let a_re = vld1q_f32(accum_re.as_ptr().add(i));
                let a_im = vld1q_f32(accum_im.as_ptr().add(i));
                vst1q_f32(accum_re.as_mut_ptr().add(i), vaddq_f32(a_re, re_inc));
                vst1q_f32(accum_im.as_mut_ptr().add(i), vaddq_f32(a_im, im_inc));
                i += super::LANES;
            }
        }
        while i < n {
            let hr = h_re[i];
            let hi = h_im[i];
            let irr = ir_re[i];
            let iri = ir_im[i];
            accum_re[i] += hr * irr - hi * iri;
            accum_im[i] += hr * iri + hi * irr;
            i += 1;
        }
    }
}

/// `simd128` kernels, used on `wasm32`.
///
/// The `target_feature` attribute is what makes these compile even without a
/// global `+simd128` rustflag; calling them requires SIMD-capable WASM, which
/// every current browser provides.
#[cfg(target_arch = "wasm32")]
mod wasm {
    use core::arch::wasm32::*;

    /// Multiplies every element by `gain`.
    ///
    /// # Safety
    ///
    /// Reads and writes only within `samples`.
    #[target_feature(enable = "simd128")]
    pub unsafe fn scale_in_place(samples: &mut [f32], gain: f32) {
        let n = samples.len();
        let vector_gain = f32x4_splat(gain);
        let mut i = 0;
        while i + super::LANES <= n {
            // SAFETY: `i + LANES <= n`.
            unsafe {
                let v = v128_load(samples.as_ptr().add(i) as *const v128);
                v128_store(
                    samples.as_mut_ptr().add(i) as *mut v128,
                    f32x4_mul(v, vector_gain),
                );
            }
            i += super::LANES;
        }
        while i < n {
            samples[i] *= gain;
            i += 1;
        }
    }

    /// `out = a * b`.
    ///
    /// # Safety
    ///
    /// All three slices must be the same length.
    #[target_feature(enable = "simd128")]
    pub unsafe fn mul_into(out: &mut [f32], a: &[f32], b: &[f32]) {
        let n = out.len();
        let mut i = 0;
        while i + super::LANES <= n {
            // SAFETY: all three slices are equal length.
            unsafe {
                let va = v128_load(a.as_ptr().add(i) as *const v128);
                let vb = v128_load(b.as_ptr().add(i) as *const v128);
                v128_store(out.as_mut_ptr().add(i) as *mut v128, f32x4_mul(va, vb));
            }
            i += super::LANES;
        }
        while i < n {
            out[i] = a[i] * b[i];
            i += 1;
        }
    }

    /// `out = input * mul + add`.
    ///
    /// # Safety
    ///
    /// `out` and `input` must be the same length; each is read/written only
    /// within its own bounds.
    #[target_feature(enable = "simd128")]
    pub unsafe fn affine_in_place(out: &mut [f32], input: &[f32], mul: f32, add: f32) {
        let n = out.len();
        let vector_mul = f32x4_splat(mul);
        let vector_add = f32x4_splat(add);
        let mut i = 0;
        while i + super::LANES <= n {
            // SAFETY: `i + LANES <= n`.
            unsafe {
                let v = v128_load(input.as_ptr().add(i) as *const v128);
                let r = f32x4_add(f32x4_mul(v, vector_mul), vector_add);
                v128_store(out.as_mut_ptr().add(i) as *mut v128, r);
            }
            i += super::LANES;
        }
        while i < n {
            out[i] = input[i] * mul + add;
            i += 1;
        }
    }

    /// Complex multiply-accumulate.
    ///
    /// # Safety
    ///
    /// Every slice must be at least `h_re.len()` long.
    #[target_feature(enable = "simd128")]
    pub unsafe fn complex_mac(
        accum_re: &mut [f32],
        accum_im: &mut [f32],
        h_re: &[f32],
        h_im: &[f32],
        ir_re: &[f32],
        ir_im: &[f32],
    ) {
        let n = h_re.len();
        let mut i = 0;
        while i + super::LANES <= n {
            // SAFETY: all six slices are at least `n` long.
            unsafe {
                let hr = v128_load(h_re.as_ptr().add(i) as *const v128);
                let hi = v128_load(h_im.as_ptr().add(i) as *const v128);
                let irr = v128_load(ir_re.as_ptr().add(i) as *const v128);
                let iri = v128_load(ir_im.as_ptr().add(i) as *const v128);
                let re_inc = f32x4_sub(f32x4_mul(hr, irr), f32x4_mul(hi, iri));
                let im_inc = f32x4_add(f32x4_mul(hr, iri), f32x4_mul(hi, irr));
                let a_re = v128_load(accum_re.as_ptr().add(i) as *const v128);
                let a_im = v128_load(accum_im.as_ptr().add(i) as *const v128);
                v128_store(
                    accum_re.as_mut_ptr().add(i) as *mut v128,
                    f32x4_add(a_re, re_inc),
                );
                v128_store(
                    accum_im.as_mut_ptr().add(i) as *mut v128,
                    f32x4_add(a_im, im_inc),
                );
            }
            i += super::LANES;
        }
        while i < n {
            let hr = h_re[i];
            let hi = h_im[i];
            let irr = ir_re[i];
            let iri = ir_im[i];
            accum_re[i] += hr * irr - hi * iri;
            accum_im[i] += hr * iri + hi * irr;
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random samples, so a failure is reproducible.
    fn ramp(len: usize, scale: f32, offset: f32) -> alloc::vec::Vec<f32> {
        (0..len)
            .map(|i| ((i as f32 * 0.37).sin() * scale) + offset)
            .collect()
    }

    fn reference_scale(samples: &[f32], gain: f32) -> alloc::vec::Vec<f32> {
        samples.iter().map(|s| s * gain).collect()
    }

    #[test]
    fn scale_matches_the_scalar_reference() {
        for len in [0usize, 1, 3, 4, 5, 7, 8, 9, 16, 33] {
            let mut samples = ramp(len, 2.0, -0.5);
            let expected = reference_scale(&samples, 0.75);
            scale_in_place(&mut samples, 0.75);
            for (i, (got, want)) in samples.iter().zip(expected.iter()).enumerate() {
                assert!((got - want).abs() < 1e-5, "len {len} sample {i}: {got} vs {want}");
            }
        }
    }
    #[test]
    fn affine_matches_the_scalar_reference() {
        for len in [0usize, 1, 3, 4, 5, 7, 8, 9, 16, 33] {
            let input = ramp(len, 2.0, -0.5);
            let mut out = alloc::vec![0.0_f32; len];
            affine_in_place(&mut out, &input, 1.25, 0.1);
            for (i, (&x, got)) in input.iter().zip(out.iter()).enumerate() {
                let want = x * 1.25 + 0.1;
                assert!((got - want).abs() < 1e-5, "len {len} sample {i}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn mul_into_matches_the_scalar_reference() {
        for len in [0usize, 1, 3, 4, 5, 7, 8, 9, 16, 33] {
            let a = ramp(len, 2.0, -0.5);
            let b = ramp(len, 0.5, 0.25);
            let mut out = alloc::vec![0.0_f32; len];
            mul_into(&mut out, &a, &b);
            for (i, ((&x, &y), got)) in a.iter().zip(b.iter()).zip(out.iter()).enumerate() {
                let want = x * y;
                assert!((got - want).abs() < 1e-5, "len {len} sample {i}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn complex_mac_matches_the_scalar_reference() {
        for len in [0usize, 1, 3, 4, 5, 7, 8, 9, 16, 33] {
            let h_re = ramp(len, 1.0, 0.0);
            let h_im = ramp(len, 0.5, 0.25);
            let ir_re = ramp(len, 0.8, -0.1);
            let ir_im = ramp(len, 0.3, 0.05);
            let mut accum_re = alloc::vec![0.1_f32; len];
            let mut accum_im = alloc::vec![-0.2_f32; len];

            let mut want_re = accum_re.clone();
            let mut want_im = accum_im.clone();
            scalar::complex_mac(
                &mut want_re,
                &mut want_im,
                &h_re,
                &h_im,
                &ir_re,
                &ir_im,
            );

            complex_mac(
                &mut accum_re,
                &mut accum_im,
                &h_re,
                &h_im,
                &ir_re,
                &ir_im,
            );
            for i in 0..len {
                assert!(
                    (accum_re[i] - want_re[i]).abs() < 1e-4,
                    "len {len} bin {i} real: {} vs {}",
                    accum_re[i],
                    want_re[i]
                );
                assert!(
                    (accum_im[i] - want_im[i]).abs() < 1e-4,
                    "len {len} bin {i} imag: {} vs {}",
                    accum_im[i],
                    want_im[i]
                );
            }
        }
    }

    #[test]
    fn a_shorter_accumulator_tail_is_left_alone() {
        // The convolution caller passes a full-spectrum accumulator with a
        // shorter partition source: the tail beyond the source must be
        // untouched, not zeroed or read.
        let h_re = ramp(4, 1.0, 0.0);
        let h_im = ramp(4, 1.0, 0.0);
        let ir_re = ramp(4, 1.0, 0.0);
        let ir_im = ramp(4, 1.0, 0.0);
        let mut accum_re = alloc::vec![0.0_f32; 8];
        let mut accum_im = alloc::vec![0.0_f32; 8];
        accum_re[4..].fill(9.0);
        accum_im[4..].fill(-9.0);
        complex_mac(&mut accum_re, &mut accum_im, &h_re, &h_im, &ir_re, &ir_im);
        assert_eq!(&accum_re[4..], &[9.0, 9.0, 9.0, 9.0]);
        assert_eq!(&accum_im[4..], &[-9.0, -9.0, -9.0, -9.0]);
    }
}
