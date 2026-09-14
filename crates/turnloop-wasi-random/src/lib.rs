//! Explicit, single-definition linkage for getrandom's p3-only custom backend.
//! Import this crate with `use turnloop_wasi_random as _;` in final consumers.
#![cfg(all(target_os = "wasi", target_env = "p3"))]

#[link(wasm_import_module = "wasi:random/random@0.3.0")]
unsafe extern "C" {
    #[link_name = "get-random-u64"]
    fn random_u64() -> u64;
}

// Both pinned getrandom 0.3.4 and 0.4.3 declare this same v03 symbol/signature.
// SAFETY: the unique custom backend symbol; callers supply len writable bytes,
// possibly uninitialized. Scalar WASI entropy avoids canonical list allocation.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom03::Error> {
    let mut offset = 0;
    while offset < len {
        // SAFETY: scalar WASI random import with no guest pointer arguments.
        let bytes = unsafe { random_u64() }.to_ne_bytes();
        let n = (len - offset).min(bytes.len());
        // SAFETY: this disjoint portion of the caller's output is writable; it
        // need not be initialized. No reference to uninitialized bytes is made.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), dest.add(offset), n) };
        offset += n;
    }
    Ok(())
}

/// Exercise both compatible getrandom generations through the real WASI import.
/// Also useful to verify the application's linker configuration at startup.
pub fn fill_v03(dest: &mut [u8]) -> Result<(), getrandom03::Error> {
    getrandom03::fill(dest)
}

/// getrandom 0.4 uses the same custom ABI symbol as 0.3.
pub fn fill_v04(dest: &mut [u8]) -> Result<(), getrandom04::Error> {
    getrandom04::fill(dest)
}
