use serde_json::{json, Value};
use crate::dispatch::CdpContext;


pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "enable" => {
            let session = session_id.as_ref().ok_or("Fetch.enable requires a page session")?;
            let target = ctx.sessions.get(session).cloned().ok_or("No page for session")?;
            let patterns = match params.get("patterns").and_then(Value::as_array) {
                None => Vec::new(),
                Some(patterns) => patterns.iter().map(|pattern| {
                    if pattern.get("requestStage").and_then(Value::as_str).is_some_and(|stage| stage != "Request") {
                        return Err("Response-stage interception is unsupported".to_string());
                    }
                    Ok(obscura_js::ops::InterceptPattern {
                        url_pattern: pattern.get("urlPattern").and_then(Value::as_str).unwrap_or("*").to_string(),
                        resource_type: pattern.get("resourceType").and_then(Value::as_str).map(str::to_string),
                    })
                }).collect::<Result<Vec<_>, String>>()?,
            };
            if params.get("handleAuthRequests").and_then(Value::as_bool) == Some(true) {
                return Err("Authentication interception is unsupported".into());
            }
            if let Some(registry) = &ctx.server_registry { registry.fetch_subscription(&target, Some(session)); }
            let sender = ctx.intercept_tx.clone();
            let page = ctx.get_page_mut(&target).ok_or("No page for session")?;
            page.intercept_block_patterns = patterns;
            page.enable_intercept(true);
            if let Some(sender) = sender { page.set_intercept_tx(sender); }
            Ok(json!({}))
        }
        "disable" => {
            let target = session_id.as_ref().and_then(|session| ctx.sessions.get(session)).cloned().ok_or("No page for session")?;
            if let Some(registry) = &ctx.server_registry { registry.fetch_subscription(&target, None); }
            let page = ctx.get_page_mut(&target).ok_or("No page for session")?;
            page.intercept_block_patterns.clear();
            page.enable_intercept(false);
            Ok(json!({}))
        }
        "continueRequest" | "fulfillRequest" | "failRequest" => {
            let request = params.get("requestId").and_then(Value::as_str).ok_or("Missing requestId")?;
            // Live native resolvers are consumed by the IO-side server boundary.
            // There is intentionally no completion-snapshot pause or fallback.
            Err(format!("No paused request found for requestId {request}"))
        }
        "getResponseBody" => crate::domains::network::handle("getResponseBody", params, ctx, session_id).await,
        "takeResponseBodyAsStream" => {
            let request = params.get("requestId").and_then(Value::as_str).ok_or("Missing requestId")?;
            let session = session_id.as_ref().ok_or("No page session")?;
            if !ctx.network_body_views.get(session).is_some_and(|view| view.contains(request))
                && !ctx.server_registry.as_ref().is_some_and(|registry| registry.body_visible(session, request)) {
                return Err(format!("No response body found for requestId {request}"));
            }
            let page = ctx.get_session_page(session_id).ok_or("No page for session")?;
            let native = ctx.server_registry.as_ref().and_then(|registry| registry.native_request_id(&page.id, request));
            let bytes = ctx.get_session_page_mut(session_id).and_then(|page| page.take_response_body_raw(native.as_deref().unwrap_or(request)))
                .ok_or_else(|| format!("No response body found for requestId {request}"))?;
            let stream = ctx.io_streams.insert(bytes)?;
            Ok(json!({"stream": stream}))
        }
        _ => Err(format!("Unknown Fetch method: {method}")),
    }
}
