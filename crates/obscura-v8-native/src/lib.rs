//! Thin linkage shim for the native V8 WindowProxy access-control callbacks.
//!
//! Both the `obscura-js` build script (which creates the startup snapshot) and
//! the `obscura-js` target runtime must link the same C++ callback table so
//! that V8's serialized external-reference indices agree.  Keeping the native
//! archive in a crate that both binaries depend on is what makes the build
//! script link the same object code.

use std::ffi::c_void;

extern "C" {
    fn obscura_v8_detach_global(context: *mut c_void);
    fn obscura_v8_install_window_access_check(template: *mut c_void);
    fn obscura_v8_set_failed_access_check(isolate: *mut c_void);
    fn obscura_v8_window_callback_addresses(out: *mut *const c_void, count: usize);
    fn obscura_native_window_abi() -> u32;
}

/// Detaches the global proxy of `context` so V8 can reuse it for a new
/// document from the same snapshot.
pub fn detach_global(context: *mut c_void) {
    unsafe { obscura_v8_detach_global(context) }
}

/// Installs the cross-origin named/indexed access handlers on the snapshot
/// global object template.
pub fn install_window_access_check(template: *mut c_void) {
    unsafe { obscura_v8_install_window_access_check(template) }
}

/// Installs the cross-origin failed-access handler on the isolate. This is
/// isolate state, not snapshot state: it is installed at runtime and does not
/// participate in the external-reference table.
pub fn set_failed_access_check(isolate: *mut c_void) {
    unsafe { obscura_v8_set_failed_access_check(isolate) }
}

/// Ordered V8 callback addresses used to build the extension's external
/// references in both the snapshot producer and the runtime.
pub fn window_callback_addresses(count: usize) -> Vec<*const c_void> {
    let mut addresses = vec![std::ptr::null(); count];
    unsafe { obscura_v8_window_callback_addresses(addresses.as_mut_ptr(), count) };
    addresses
}

/// Number of callbacks in the native table.  The first nine keep their
/// historical order; named enumerator and indexed descriptor/enumerator were
/// appended so cross-origin ownKeys and descriptors are populated.
pub const WINDOW_CALLBACK_COUNT: usize = 12;

/// V8 ABI flags compiled into this helper and linked at runtime:
/// bit 0 pointer compression, bit 1 31-bit Smis, bit 2 sandbox,
/// bit 3 shared pointer-compression cage, bit 4 cppgc-managed microtask queue,
/// bits 8..15 `sizeof(void*)`.
pub fn native_window_abi() -> u32 {
    unsafe { obscura_native_window_abi() }
}
