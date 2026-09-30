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
//! * S0 shipped only the version handshake. The parameter system and
//!   automation (S2) now live in [`automation`] and are exported through
//!   [`ffi::param_api`]; the DSP graph, sequencer and plugin host land in later
//!   stages behind the same ABI discipline.

#![deny(missing_docs)]

use core::ffi::c_char;
use core::panic::AssertUnwindSafe;

/// Re-export of `alloc`, so modules can name `alloc::vec::Vec` and keep working
/// unchanged if the core is ever built `no_std` for a constrained target.
///
/// The crate links `std` today, which means `alloc` is already in the extern
/// prelude; naming it here makes that dependency explicit rather than implicit
/// and keeps the `no_std` path a one-line change in the future.
extern crate alloc;

pub mod automation;

/// The C ABI surface: every `#[no_mangle] extern "C"` symbol lives under here,
/// so the exported set is auditable by reading one directory (ABI principle P1).
pub mod ffi;

/// Mixer: console topology, channel strips, sends, effect slots and metering.
///
/// Concerned with *structure and values* only — it owns no audio device and no
/// transport. See `docs/COORDINATION.md` C-004 / C-005 for the registration of
/// this module and its `ffi/` surface.
pub mod mixer;

/// ABI version of this library, as `major << 16 | minor << 8 | patch`.
///
/// Dart passes the version it was compiled against to [`zenith_version_match`]
/// so a stale `zenith_core.dll` fails loudly at startup instead of producing
/// silent audio corruption.
///
/// # Why this is 0.2.0 and not 0.1.x
///
/// S2 added the parameter and automation surface (`ffi/param_api.rs`, 40+
/// exported functions, plus the S2 structs in `ffi/types.rs`). Per
/// `docs/ABI.md` §2.2, *adding* exported functions and *appending* struct
/// fields is a backward-compatible change that bumps the minor version; no
/// existing signature, field order or enum discriminant was altered.
/// Registered as entry C-002 in `docs/COORDINATION.md`.
pub const ABI_VERSION: u32 = encode_version(0, 2, 0);

/// Status code returned by every fallible entry point.
///
/// Mirrors `ZenithStatusCode` in `docs/ABI.md` §3.2. The discriminants are part
/// of the ABI: they are written out explicitly so a future reordering of the
/// enum cannot silently change the wire values Dart already compiles against.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The call succeeded.
    Ok = 0,
    /// An argument was structurally invalid.
    InvalidArg = 1,
    /// A required pointer was null.
    NullPointer = 2,
    /// The loaded library and the caller disagree on the ABI version.
    AbiMismatch = 3,
    /// The resource already exists.
    AlreadyExists = 4,
    /// The referenced resource does not exist.
    NotFound = 5,
    /// A value was outside its permitted range.
    OutOfRange = 6,
    /// The requested connection would create a cycle in the DSP graph.
    WouldCycle = 7,
    /// A preallocated real-time pool is exhausted; the real-time path never
    /// grows.
    Capacity = 8,
    /// The object must be prepared before this call.
    NotPrepared = 9,
    /// The audio device failed.
    Device = 10,
    /// An IO operation failed.
    Io = 11,
    /// The operation is unsupported on this platform or tier.
    Unsupported = 12,
    /// A Rust panic was caught at the FFI boundary. The affected object is
    /// poisoned: destroy and rebuild it rather than retrying.
    Panicked = 13,
    /// The object is busy and cannot service the call yet.
    Busy = 14,
    /// An invariant was violated; the cause is not one of the above.
    Internal = 15,
}

impl Status {
    /// The raw ABI discriminant.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// `true` when the status represents success.
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Status::Ok)
    }
}

/// Runs `body` at the FFI boundary, converting a caught panic into
/// [`Status::Panicked`].
///
/// This is the single implementation of ABI principle P4 (`docs/ABI.md` §4.2):
/// a Rust panic unwinding into the Dart VM is undefined behaviour, so every
/// `extern "C"` entry point must funnel through here. It works only because the
/// release profile does **not** set `panic = "abort"` — see `Cargo.toml`.
///
/// A panic poisons the affected object: the caller must destroy and rebuild it
/// rather than retrying, because the object may be half-updated
/// (`docs/ABI.md` §3.2).
///
/// `AssertUnwindSafe` is sound here because, on the panic path, the object is
/// documented as poisoned and every subsequent call on it is rejected — no
/// partially mutated state is ever observed as valid.
#[must_use]
pub fn guard(body: impl FnOnce() -> Status) -> Status {
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(status) => status,
        Err(_) => Status::Panicked,
    }
}

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
/// string, e.g. `"0.2.0"`.
///
/// The pointer is valid for the entire lifetime of the process and must not be
/// freed by the caller.
///
/// # Safety
///
/// The returned pointer is `'static` and must only be read, never written or
/// freed.
#[no_mangle]
pub extern "C" fn zenith_version_string() -> *const c_char {
    // A byte-string literal with a trailing NUL, so no allocation or `CString`
    // is needed and the pointer is genuinely 'static.
    concat!("0.2.0", "\0").as_ptr() as *const c_char
}

/// Panics on purpose to prove the panic firewall works.
///
/// This is the S1.0 acceptance probe for prerequisite A. It is a *diagnostic*
/// export, not part of the product ABI, and it exists so the guarantee can be
/// tested end to end rather than asserted in a comment: call it and it must
/// return [`Status::Panicked`] with the process still alive.
///
/// When `with_payload` is `0` the panic carries a static message; when it is
/// non-zero the payload is a `String`, covering the allocating panic path that
/// real DSP code will actually hit.
///
/// # Safety
///
/// No preconditions — the function takes no pointers and reads no memory. It is
/// safe to call from any thread and its only effect is the returned status.
#[no_mangle]
pub extern "C" fn zenith_panic_probe(with_payload: u32) -> i32 {
    guard(|| {
        if with_payload == 0 {
            panic!("zenith_panic_probe: intentional static-message panic");
        }
        panic!("zenith_panic_probe: intentional owned-payload panic {}", 42);
    })
    .code()
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
        assert_eq!(s.to_str().unwrap(), "0.2.0");
    }

    #[test]
    fn abi_version_encodes_0_2_0() {
        assert_eq!(ABI_VERSION >> 16, 0, "major");
        assert_eq!((ABI_VERSION >> 8) & 0xFF, 2, "minor");
        assert_eq!(ABI_VERSION & 0xFF, 0, "patch");
    }

    #[test]
    fn the_version_string_agrees_with_the_encoded_stamp() {
        // A mismatch here is how a "rebuilt but stale DLL" hides: the numeric
        // check passes while the human-readable string lies to whoever is
        // debugging.
        let text = format!(
            "{}.{}.{}",
            ABI_VERSION >> 16,
            (ABI_VERSION >> 8) & 0xFF,
            ABI_VERSION & 0xFF
        );
        // SAFETY: the function returns a 'static NUL-terminated literal.
        let reported = unsafe { CStr::from_ptr(zenith_version_string()) }
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(reported, text);
    }

    // ── Prerequisite A: the panic firewall must actually catch ──
    //
    // These tests are what makes the removal of `panic = "abort"` verifiable
    // rather than a claim: under `abort` the first one kills the test process
    // instead of returning a status code.

    #[test]
    fn status_codes_match_the_published_abi() {
        // These discriminants are compiled into Dart already; a silent shift
        // would be an ABI break (docs/ABI.md §2.2).
        assert_eq!(Status::Ok.code(), 0);
        assert_eq!(Status::InvalidArg.code(), 1);
        assert_eq!(Status::NullPointer.code(), 2);
        assert_eq!(Status::AbiMismatch.code(), 3);
        assert_eq!(Status::Panicked.code(), 13);
        assert_eq!(Status::Internal.code(), 15);
        assert!(Status::Ok.is_ok());
        assert!(!Status::Panicked.is_ok());
    }

    #[test]
    fn guard_turns_a_panic_into_a_status_code() {
        let status = guard(|| panic!("intentional panic inside the guard"));
        assert_eq!(status, Status::Panicked);
        assert_eq!(status.code(), 13);
    }

    #[test]
    fn guard_passes_through_a_successful_result() {
        assert_eq!(guard(|| Status::Ok), Status::Ok);
        assert_eq!(guard(|| Status::NotFound), Status::NotFound);
    }

    #[test]
    fn panic_probe_returns_panicked_for_static_and_owned_payloads() {
        // Static-message payload.
        assert_eq!(zenith_panic_probe(0), Status::Panicked.code());
        // Allocating `String` payload — the path real DSP code hits.
        assert_eq!(zenith_panic_probe(1), Status::Panicked.code());
    }

    #[test]
    fn process_survives_a_panic_at_the_ffi_boundary() {
        // Reaching the assertions after the probe is the actual proof: under
        // `panic = "abort"` this test binary would already be dead. Calling it
        // repeatedly also shows the boundary is re-entrant, not one-shot.
        for _ in 0..8 {
            assert_eq!(zenith_panic_probe(0), Status::Panicked.code());
            // The library is still fully functional afterwards.
            assert_eq!(zenith_version(), ABI_VERSION);
            assert_eq!(zenith_version_match(ABI_VERSION), 1);
        }
    }
}
