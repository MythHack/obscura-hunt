//! Native WindowProxy access-control seam shared by the snapshot producer and
//! the runtime.
//!
//! The snapshot's global template must already carry the access-check and
//! named/indexed handlers: `Context::from_snapshot` cannot inject a template
//! when it restores a context.  Both `build.rs` and the runtime therefore use
//! this module to assemble the extension, and both must link the same C++
//! callback table so V8's external-reference indices agree.

use std::borrow::Cow;
use std::ffi::c_void;

/// Builds the extension whose global-template middleware installs the native
/// cross-origin Window handlers in the snapshot.
pub fn native_window_extension() -> deno_core::Extension {
    deno_core::Extension {
        name: "obscura_native_window",
        external_references: Cow::Owned(external_references()),
        global_template_middleware: Some(global_template_middleware),
        ..Default::default()
    }
}

fn external_references() -> Vec<deno_core::v8::ExternalReference> {
    obscura_v8_native::window_callback_addresses(obscura_v8_native::WINDOW_CALLBACK_COUNT)
        .into_iter()
        .map(|address| deno_core::v8::ExternalReference {
            pointer: address as *mut c_void,
        })
        .collect()
}

fn global_template_middleware<'s, 'i>(
    _scope: &mut deno_core::v8::PinScope<'s, 'i, ()>,
    template: deno_core::v8::Local<'s, deno_core::v8::ObjectTemplate>,
) -> deno_core::v8::Local<'s, deno_core::v8::ObjectTemplate> {
    let ptr = &*template as *const deno_core::v8::ObjectTemplate as *mut c_void;
    obscura_v8_native::install_window_access_check(ptr);
    template
}


unsafe extern "C" {
    // The shim takes the engine isolate pointer. `UnsafeRawIsolatePtr` is
    // repr(transparent) over that pointer; a pointer to Rust's `Isolate`
    // wrapper would address the wrapper's pointer holder instead.
    fn obscura_v8_set_failed_access_check(isolate: deno_core::v8::UnsafeRawIsolatePtr);
}

/// Installs the isolate-level failed-access handler. The snapshot only carries
/// the named/indexed handlers; non-property operations such as
/// `Object.setPrototypeOf`, `Object.freeze` and `Reflect.ownKeys` report through
/// this runtime callback, whose handler throws the target realm's SecurityError.
pub fn install_failed_access_check(isolate: &mut deno_core::v8::Isolate) {
    let raw = unsafe { isolate.as_raw_isolate_ptr() };
    unsafe { obscura_v8_set_failed_access_check(raw) }
}

/// Index of deno_core's runtime context inside the startup snapshot.
///
/// V8's `Context::FromSnapshot(index)` deserializes blob slot `index + 1`; the
/// producer stores the untouched default context in slot 0 and
/// `JsRuntimeForSnapshot`'s main realm (the one created through this crate's
/// `global_template_middleware`) in slot 1. The public index for that realm is
/// therefore **0**. Deno's own `create_context` tries 1 first for embedders
/// that add an extra context and falls back to 0, which is why 1 always missed
/// here. Index 0 is the only slot that carries the native handlers and the
/// bundle field; never fall back to the empty default context.
pub(crate) const SNAPSHOT_INDEX: usize = 0;
