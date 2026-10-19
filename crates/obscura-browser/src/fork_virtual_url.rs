//! Fork-only: adopt a URL the page routed to itself.
//!
//! A single page app answers a click by calling `history.pushState` and
//! rendering the next view in place. `bootstrap.js` (and history/location
//! glue) writes the reflected URL through the native `op_set_reflected_url`;
//! no JS global remains on the Rust side. `sync_virtual_url` reads it directly
//! through `js.reflected_url(0)`, so the page no longer reports the old
//! document URL after an in-place route.
//!
//! Ported from fork commit `d7dca7a`. Kept out of `page.rs` so an upstream
//! rewrite of that file does not touch it; Rust allows an inherent impl in any
//! module of the defining crate, so the call site still reads
//! `self.sync_virtual_url()`.

use url::Url;

use crate::page::Page;

impl Page {
    /// Adopt a URL the page routed to itself, without fetching anything.
    ///
    /// Returns whether the URL changed.
    pub fn sync_virtual_url(&mut self) -> bool {
        let Some(js) = self.js.as_ref() else {
            return false;
        };
        // The reflected URL lives in native state now. Reading it through a
        // script would let a page-defined getter change what the host sees.
        let Some(virtual_url) = js.reflected_url(0) else {
            return false;
        };
        let virtual_url = virtual_url.as_str();
        let Ok(parsed) = Url::parse(virtual_url) else {
            return false;
        };
        if self.url.as_ref() == Some(&parsed) {
            return false;
        }
        // A web document must not become a file:// page without loading one:
        // page.url keys the file:// sub-resource gate (#1069).
        if crate::page::cross_scheme_to_file(&self.url_string(), virtual_url) {
            return false;
        }
        self.url = Some(parsed);
        self.push_same_document_history(self.url_string());
        true
    }
}
