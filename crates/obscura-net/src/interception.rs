use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::{ObscuraNetError, ResourceRequest, Response};

#[derive(Debug)]
pub enum InterceptResolution {
    Continue {
        url: Option<String>,
        method: Option<String>,
        headers: Option<HashMap<String, String>>,
        body: Option<Vec<u8>>,
    },
    Fulfill { status: u16, headers: HashMap<String, String>, body: Vec<u8> },
    Fail { reason: String },
}

pub struct InterceptedRequest {
    pub request_id: String,
    pub page_id: String,
    pub frame_id: u32,
    pub parent_frame_id: Option<u32>,
    pub frame_generation: u64,
    pub body: Option<Vec<u8>>,
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub resource_type: String,
    pub resolver: oneshot::Sender<InterceptResolution>,
}

/// A request-stage selector matches URL and native resource type together.
#[derive(Debug, Clone)]
pub struct InterceptPattern {
    pub url_pattern: String,
    pub resource_type: Option<String>,
}

impl InterceptPattern {
    pub fn matches(&self, url: &str, resource_type: &str) -> bool {
        self.resource_type.as_deref().is_none_or(|kind| kind == resource_type)
            && url_matches_cdp_pattern(&self.url_pattern, url)
    }
}

#[derive(Clone, Default)]
pub struct RequestInterception {
    pub page_id: String,
    pub enabled: bool,
    pub patterns: Vec<InterceptPattern>,
    pub blocked: Vec<String>,
    pub sender: Option<mpsc::UnboundedSender<InterceptedRequest>>,
    pub observer: Option<std::sync::Arc<dyn Fn(InterceptedRequest) + Send + Sync>>,
}

impl RequestInterception {
    pub async fn resolve(
        &self,
        request_id: &str,
        url: &mut Url,
        method: &mut String,
        headers: &mut HashMap<String, String>,
        body: &mut Option<Vec<u8>>,
        request: &ResourceRequest,
    ) -> Result<Option<Response>, ObscuraNetError> {
        if self.blocked.iter().any(|pattern| url_matches_cdp_pattern(pattern, url.as_str())) {
            return Err(ObscuraNetError::Blocked(url.to_string()));
        }
        if !self.enabled { return Ok(None); }
        let resource_type = match request.resource_type {
            crate::ResourceType::Document => "Document",
            crate::ResourceType::Script => "Script",
            crate::ResourceType::Stylesheet => "Stylesheet",
            crate::ResourceType::Image => "Image",
            crate::ResourceType::Font => "Font",
            crate::ResourceType::Xhr => "XHR",
            crate::ResourceType::Fetch => "Fetch",
            crate::ResourceType::Other => "Other",
        };
        if !self.patterns.is_empty()
            && !self.patterns.iter().any(|pattern| pattern.matches(url.as_str(), resource_type)) {
            return Ok(None);
        }
        let (resolver, receiver) = oneshot::channel();
        let intercepted = InterceptedRequest {
            request_id: request_id.to_string(),
            page_id: self.page_id.clone(), frame_id: request.frame_id,
            parent_frame_id: request.parent_frame_id,
            frame_generation: request.frame_generation, body: body.clone(),
            url: url.to_string(), method: method.clone(), headers: headers.clone(),
            resource_type: resource_type.to_owned(), resolver,
        };
        if let Some(observer) = &self.observer {
            observer(intercepted);
        } else {
            self.sender.as_ref().ok_or_else(|| ObscuraNetError::Blocked("Interception consumer is unavailable".into()))?
                .send(intercepted).map_err(|_| ObscuraNetError::Blocked("Interception consumer disconnected".into()))?;
        }
        let resolution = tokio::time::timeout(std::time::Duration::from_secs(30), receiver).await
            .map_err(|_| ObscuraNetError::Blocked("Interception decision timed out".into()))?
            .map_err(|_| ObscuraNetError::Blocked("Interception consumer disconnected".into()))?;
        match resolution {
            InterceptResolution::Continue { url: replacement, method: replacement_method, headers: replacement_headers, body: replacement_body } => {
                if let Some(replacement) = replacement {
                    *url = Url::parse(&replacement).map_err(|error| ObscuraNetError::Network(format!("Invalid intercepted URL: {error}")))?;
                }
                if let Some(replacement) = replacement_method { *method = replacement; }
                if let Some(replacement) = replacement_headers { *headers = replacement; }
                if let Some(replacement) = replacement_body { *body = Some(replacement); }
                Ok(None)
            }
            InterceptResolution::Fulfill { status, headers, body } => {
                if body.len() > request.max_response_bytes {
                    return Err(crate::client::response_too_large(url, request.max_response_bytes));
                }
                let mut normalized_headers = HashMap::with_capacity(headers.len());
                for (mut name, value) in headers {
                    name.make_ascii_lowercase();
                    crate::client::merge_response_header(&mut normalized_headers, name, value);
                }
                Ok(Some(Response { url: url.clone(), status, headers: normalized_headers, body, redirected_from: Vec::new() }))
            }
            InterceptResolution::Fail { reason } => Err(ObscuraNetError::Blocked(reason)),
        }
    }
}

/// CDP glob syntax, shared by native and JavaScript producers.
pub fn url_matches_cdp_pattern(pattern: &str, url: &str) -> bool {
    if pattern == "*" { return true; }
    let mut pattern = pattern.chars();
    let mut url = url.chars();
    let mut star = None;
    loop {
        let mut next_pattern = pattern.clone();
        match next_pattern.next() {
            Some('*') => { pattern = next_pattern; star = Some((pattern.clone(), url.clone())); }
            Some('?') if url.next().is_some() => { pattern = next_pattern; }
            Some(mut expected) => {
                if expected == '\\' { expected = next_pattern.next().unwrap_or('\\'); }
                let mut next_url = url.clone();
                if next_url.next() == Some(expected) { pattern = next_pattern; url = next_url; continue; }
                if let Some((saved_pattern, saved_url)) = &mut star {
                    if saved_url.next().is_some() { pattern = saved_pattern.clone(); url = saved_url.clone(); continue; }
                }
                return false;
            }
            None if url.clone().next().is_none() => return true,
            _ => {
                if let Some((saved_pattern, saved_url)) = &mut star {
                    if saved_url.next().is_some() { pattern = saved_pattern.clone(); url = saved_url.clone(); continue; }
                }
                return false;
            }
        }
    }
}
