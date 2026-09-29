//! ZENITH AUDIO native core — C ABI surface consumed by Dart through
//! `dart:ffi`.
//!
//! # Contract
//!
//! * Every symbol exported here is `#[no_mangle] extern "C"` and declared in
//!   `docs/ABI.md`. Nothing outside that document may be called from Dart.
//! * Functions taking or returning raw pointers validate them and never panic
//!   across the FFI boundary: a Rust panic unwinding into the Dart VM is
//!   undefined behaviour, so every entry point is `catch_unwind`-guarded or
//!   provably panic-free.
//! * S0 ships only the version handshake. The DSP graph, sequencer and plugin
//!   host land in later stages behind the same ABI discipline.

#![deny(missing_docs)]

/// ABI version of this library, as `major << 16 | minor << 8 | patch`.
///
/// Dart passes the version it was compiled against to [`zenith_version_match`]
/// so a stale `zenith_core.dll` fails loudly at startup instead of producing
/// silent audio corruption.
pub const ABI_VERSION: u32 = encode_version(0, 1, 0);

/// Packs a semantic version into the ABI stamp layout.
///
/// A helper rather than an inline expression so the shift widths stay in one
/// place as the ABI grows.
#[must_use]
pub const fn encode_version(major: u8, minor: u8, patch: u8) -> u32 {
    ((major as u32) << 16) | ((minor as u32) << 8) | (patch as u32)
}

/// Returns the ABI version of the loaded native library.
///
/// This is the S0 link probe: if `flutter build windows` links the Rust
/// staticlib correctly, Dart can resolve and call this symbol.
///
/// # Safety
///
/// No preconditions — the function reads no memory and takes no arguments.
#[no_mangle]
pub extern "C" fn zenith_version() -> u32 {
    ABI_VERSION
}

/// Returns `1` when `expected` matches this library's ABI version, else `0`.
///
/// Returning a status code rather than panicking keeps the failure inside
/// Dart's control flow, where it can be surfaced to the user.
///
/// # Safety
///
/// No preconditions — the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_version_match(expected: u32) -> u32 {
    u32::from(expected == ABI_VERSION)
}

/// Returns a NUL-terminated, statically allocated human-readable version
/// string, e.g. `"0.1.0"`.
///
/// The pointer is valid for the entire lifetime of the process and must not be
/// freed by the caller.
///
/// # Safety
///
/// The returned pointer is `'static` and must only be read, never written or
/// freed.
#[no_mangle]
pub extern "C" fn zenith_version_string() -> *const core::ffi::c_char {
    // A byte-string literal with a trailing NUL, so no allocation or `CString`
    // is needed and the pointer is genuinely 'static.
    concat!("0.1.0", "\0").as_ptr() as *const core::ffi::c_char
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;

    #[test]
    fn version_is_a_nonzero_abi_stamp() {
        assert_ne!(zenith_version(), 0);
        assert_eq!(zenith_version(), ABI_VERSION);
    }

    #[test]
    fn version_match_accepts_exact_and_rejects_others() {
        assert_eq!(zenith_version_match(ABI_VERSION), 1);
        assert_eq!(zenith_version_match(ABI_VERSION.wrapping_add(1)), 0);
        assert_eq!(zenith_version_match(0), 0);
    }

    #[test]
    fn version_string_round_trips_through_c_str() {
        // SAFETY: the function returns a 'static NUL-terminated literal.
        let s = unsafe { CStr::from_ptr(zenith_version_string()) };
        assert_eq!(s.to_str().unwrap(), "0.1.0");
    }

    #[test]
    fn abi_version_encodes_0_1_0() {
        assert_eq!(ABI_VERSION >> 16, 0, "major");
        assert_eq!((ABI_VERSION >> 8) & 0xFF, 1, "minor");
        assert_eq!(ABI_VERSION & 0xFF, 0, "patch");
    }
}
