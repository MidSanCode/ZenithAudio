//! C ABI surface for the offline audio-edit algorithms (PLAN §3.S8).
//!
//! # Memory contract
//!
//! Inputs are supplied by the caller and only read; Rust never keeps the
//! pointer past the call. Outputs are **Rust-allocated** and returned as
//! `*mut f32`; the caller must release them with [`zenith_buffer_free`] using
//! the length written to `*out_count` (ABI principle P3, same rule as
//! [`crate::ffi::render_api::zenith_render_offline`]).
//!
//! # Real-time safety
//!
//! None of these functions is real-time safe: they allocate and run heavy
//! transforms. Call them on a control or worker thread, never from the audio
//! callback.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ptr;

use crate::edit::{crossfade as crossfade_impl, time_stretch, transient};
use crate::edit::FadeCurve;
use crate::ffi::render_api::zenith_buffer_free;
use crate::guard;
use crate::Status;

/// Copies `len` floats out of `pointer`, or `None` when the pointer is null.
///
/// # Safety
///
/// The caller promises `pointer` holds `len` readable floats.
unsafe fn input_slice<'a>(pointer: *const f32, len: usize) -> Option<&'a [f32]> {
    if pointer.is_null() {
        return None;
    }
    if len == 0 {
        return Some(&[]);
    }
    // SAFETY: the caller promises the buffer has `len` readable floats.
    Some(unsafe { core::slice::from_raw_parts(pointer, len) })
}

/// Moves a Rust `Vec<f32>` to the caller and writes its length to `out_count`.
///
/// Returns the pointer, or null when `out_count` is null (nothing is leaked
/// because the caller cannot learn the length to free it, so the Vec is
/// dropped).
fn into_out(vec: alloc::vec::Vec<f32>, out_count: *mut usize) -> *mut f32 {
    if out_count.is_null() {
        return ptr::null_mut();
    }
    let count = vec.len();
    let mut boxed = vec.into_boxed_slice();
    let pointer = boxed.as_mut_ptr();
    core::mem::forget(boxed);
    // SAFETY: `out_count` was checked non-null.
    unsafe { ptr::write(out_count, count) };
    pointer
}

/// Stretches `[input, input + input_len)` in time by `factor`, preserving pitch.
///
/// Writes the new length to `*out_count`. Returns a Rust-owned buffer the
/// caller must free with [`zenith_buffer_free`], or null on a null out-count.
///
/// # Safety
///
/// `input` must hold `input_len` readable floats; `out_count` writable.
#[no_mangle]
pub extern "C" fn zenith_time_stretch(
    input: *const f32,
    input_len: usize,
    factor: f32,
    out_count: *mut usize,
) -> *mut f32 {
    let mut result: *mut f32 = ptr::null_mut();
    let status = guard(|| {
        // SAFETY: the caller promises the input contract.
        let Some(slice) = (unsafe { input_slice(input, input_len) }) else {
            return Status::NullPointer;
        };
        result = into_out(time_stretch(slice, factor), out_count);
        Status::Ok
    });
    if status.is_ok() {
        result
    } else {
        ptr::null_mut()
    }
}

/// Shifts `[input, input + input_len)` in pitch by `semitones`, keeping length.
///
/// # Safety
///
/// `input` must hold `input_len` readable floats; `out_count` writable.
#[no_mangle]
pub extern "C" fn zenith_pitch_shift(
    input: *const f32,
    input_len: usize,
    semitones: f32,
    out_count: *mut usize,
) -> *mut f32 {
    let mut result: *mut f32 = ptr::null_mut();
    let status = guard(|| {
        // SAFETY: the caller promises the input contract.
        let Some(slice) = (unsafe { input_slice(input, input_len) }) else {
            return Status::NullPointer;
        };
        result = into_out(crate::edit::pitch_shift(slice, semitones), out_count);
        Status::Ok
    });
    if status.is_ok() {
        result
    } else {
        ptr::null_mut()
    }
}

/// Detects transients in `[input, input + input_len)`.
///
/// Writes up to `out_capacity` sample positions (as `u32`) into `out` and the
/// actual count into `*out_count`. Unlike the buffer-returning functions this
/// writes into a caller-provided array, because the result is bounded and small;
/// a caller that does not know the bound can pass a generous capacity and read
/// `*out_count`.
///
/// # Safety
///
/// `input` must hold `input_len` floats; `out` must hold `out_capacity` `u32`s;
/// `out_count` writable.
#[no_mangle]
pub extern "C" fn zenith_detect_transients(
    input: *const f32,
    input_len: usize,
    out: *mut u32,
    out_capacity: usize,
    out_count: *mut usize,
) -> i32 {
    guard(|| {
        if out_count.is_null() {
            return Status::NullPointer;
        }
        // SAFETY: the caller promises the input contract.
        let Some(slice) = (unsafe { input_slice(input, input_len) }) else {
            return Status::NullPointer;
        };
        let positions = transient::detect_transients(slice);
        let written = positions.len().min(out_capacity);
        if !out.is_null() && written > 0 {
            // SAFETY: `out` holds `out_capacity` u32s and `written <= capacity`.
            let dest = unsafe { core::slice::from_raw_parts_mut(out, out_capacity) };
            for (i, position) in positions.iter().take(written).enumerate() {
                dest[i] = *position as u32;
            }
        }
        // SAFETY: checked non-null above.
        unsafe { ptr::write(out_count, written) };
        Status::Ok
    })
    .code()
}

/// Crossfades `a` into `b` over `fade` samples with the given curve.
///
/// `curve`: `0` linear, `1` equal-power. Returns a Rust-owned buffer the caller
/// must free with [`zenith_buffer_free`].
///
/// # Safety
///
/// Both inputs must hold their stated lengths; `out_count` writable.
#[no_mangle]
pub extern "C" fn zenith_crossfade(
    a: *const f32,
    a_len: usize,
    b: *const f32,
    b_len: usize,
    fade: usize,
    curve: u32,
    out_count: *mut usize,
) -> *mut f32 {
    if fade > 0 && (a.is_null() || b.is_null()) {
        return ptr::null_mut();
    }
    let mut result: *mut f32 = ptr::null_mut();
    let status = guard(|| {
        // SAFETY: the caller promises both input contracts.
        let (slice_a, slice_b) = unsafe {
            (input_slice(a, a_len), input_slice(b, b_len))
        };
        let (Some(slice_a), Some(slice_b)) = (slice_a, slice_b) else {
            return Status::NullPointer;
        };
        let curve = match curve {
            1 => FadeCurve::EqualPower,
            _ => FadeCurve::Linear,
        };
        result = into_out(crossfade_impl(slice_a, slice_b, fade, curve), out_count);
        Status::Ok
    });
    if status.is_ok() {
        result
    } else {
        ptr::null_mut()
    }
}

/// Re-exported so the header has one free function for every Rust-allocated
/// buffer this module returns. Functionally [`crate::ffi::render_api::zenith_buffer_free`].
///
/// # Safety
///
/// As [`crate::ffi::render_api::zenith_buffer_free`].
#[no_mangle]
pub extern "C" fn zenith_edit_buffer_free(buffer: *mut f32, frames: usize) {
    zenith_buffer_free(buffer, frames);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(len: usize, hz: f32) -> alloc::vec::Vec<f32> {
        (0..len)
            .map(|i| {
                crate::effects::util::dsp::sin_poly(core::f32::consts::TAU * hz * i as f32 / SR)
            })
            .collect()
    }

    #[test]
    fn time_stretch_doubles_the_length() {
        let input = sine(4_000, 440.0);
        let mut count: usize = 0;
        let out = zenith_time_stretch(input.as_ptr(), input.len(), 2.0, &mut count);
        assert!(!out.is_null());
        assert!((count as i64 - 8_000).abs() <= 1, "got {count}");
        zenith_edit_buffer_free(out, count);
    }

    #[test]
    fn a_null_input_is_refused() {
        let mut count: usize = 0;
        let out = zenith_time_stretch(ptr::null(), 0, 2.0, &mut count);
        assert!(out.is_null());
    }

    #[test]
    fn pitch_shift_preserves_length() {
        let input = sine(4_000, 440.0);
        let mut count: usize = 0;
        let out = zenith_pitch_shift(input.as_ptr(), input.len(), 12.0, &mut count);
        assert!(!out.is_null());
        let ratio = count as f64 / input.len() as f64;
        assert!((ratio - 1.0).abs() < 0.05, "ratio {ratio}");
        zenith_edit_buffer_free(out, count);
    }

    #[test]
    fn transient_detection_writes_indices_and_count() {
        let mut input = alloc::vec![0.0f32; 16_000];
        for p in [0, 4_000, 8_000, 12_000] {
            input[p] = 1.0;
        }
        let mut out = alloc::vec![0u32; 64];
        let mut count: usize = 0;
        let code = zenith_detect_transients(
            input.as_ptr(),
            input.len(),
            out.as_mut_ptr(),
            out.len(),
            &mut count,
        );
        assert_eq!(code, Status::Ok.code());
        assert!(count >= 1, "expected transients, got {count}");
    }

    #[test]
    fn transient_detection_with_a_null_out_only_reports_the_count() {
        let input: alloc::vec::Vec<f32> = (0..16_000).map(|i| (i % 64 == 0) as u32 as f32).collect();
        let mut count: usize = 0;
        let code = zenith_detect_transients(
            input.as_ptr(),
            input.len(),
            ptr::null_mut(),
            0,
            &mut count,
        );
        assert_eq!(code, Status::Ok.code());
    }

    #[test]
    fn crossfade_concatenates_with_the_expected_length() {
        let a = alloc::vec![1.0f32; 100];
        let b = alloc::vec![2.0f32; 80];
        let mut count: usize = 0;
        let out = zenith_crossfade(
            a.as_ptr(),
            a.len(),
            b.as_ptr(),
            b.len(),
            40,
            1,
            &mut count,
        );
        assert!(!out.is_null());
        assert_eq!(count, 140);
        zenith_edit_buffer_free(out, count);
    }
}
