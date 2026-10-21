use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use obscura_net::Response;
use url::Url;

use super::{Page, PageError, StoredResponseBody, decode_data_uri};

impl Page {
    /// Fetch a native document through the page's selected transport. The
    /// transport owns the single pre-I/O interception decision and callbacks.
    pub(super) async fn fetch_document(&mut self, url: &Url, method: &str, body: &str) -> Result<Response, PageError> {
        self.ensure_document_navigation_live()?;
        let response = if url.scheme() == "data" {
            let content_type = url.as_str().strip_prefix("data:").and_then(|value| value.split(',').next())
                .unwrap_or("text/html").split(';').next().unwrap_or("text/html").to_string();
            Ok(Response {
                url: url.clone(), status: 200, headers: HashMap::from([("content-type".into(), content_type)]),
                body: decode_data_uri(url.as_str()).unwrap_or_default(), redirected_from: Vec::new(),
            })
        } else {
            let mut request = obscura_net::ResourceRequest::navigation();
            request.initiator = self.window_navigation.as_ref()
                .and_then(|navigation| Url::parse(&navigation.source_url).ok())
                .or_else(|| Url::parse(&self.referrer).ok());
            request.referrer = Url::parse(&self.referrer).ok();
            request.referrer_policy = self.referrer_policy.clone();
            request.frame_generation = self.window_document_generation;
            self.do_fetch_with_method(url, method, (method == "POST").then(|| body.as_bytes().to_vec()), request).await
        };
        self.ensure_document_navigation_live()?;
        match response {
            Ok(response) => {
                self.record_network_event_with_body(response.url.as_str(), method, "Document", response.status,
                    &response.headers, &response.body, false);
                Ok(response)
            }
            Err(error) => {
                self.lifecycle = crate::lifecycle::LifecycleState::Failed;
                Err(PageError::NetworkError(error.to_string()))
            }
        }
    }

    fn ensure_document_navigation_live(&self) -> Result<(), PageError> {
        if self.window_navigation.as_ref().is_some_and(|navigation| {
            !self.js.as_ref().is_some_and(|js| js.window_navigation_valid(navigation))
        }) {
            return Err(PageError::NetworkError("Native window navigation was superseded".into()));
        }
        Ok(())
    }

    pub(super) fn store_network_response_body(&mut self, request_id: String,
        response_headers: &HashMap<String, String>, body: &[u8], base64_encoded: bool) {
        self.store_response_body_with(request_id, body.len(), || {
            if !base64_encoded {
                if let Some(text) = obscura_net::encoding::decode_devtools_response_body(body, response_headers.get("content-type").map(String::as_str)) {
                    return StoredResponseBody { body: text, base64_encoded: false };
                }
            }
            StoredResponseBody { body: BASE64.encode(body), base64_encoded: true }
        });
    }
}
