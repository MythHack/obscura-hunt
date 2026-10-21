use serde_json::{json, Value};

use crate::dispatch::CdpContext;


fn mouse_button_code(button: &str) -> u8 {
    match button {
        "middle" => 1,
        "right" => 2,
        "back" => 3,
        "forward" => 4,
        _ => 0,
    }
}

fn mouse_button_mask(button: &str) -> u64 {
    match button {
        "right" => 2,
        "middle" => 4,
        "back" => 8,
        "forward" => 16,
        "none" => 0,
        _ => 1,
    }
}


pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "dispatchMouseEvent" => {
            let event_type = params.get("type").and_then(Value::as_str).unwrap_or("");
            let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let button = params.get("button").and_then(Value::as_str).unwrap_or("left");
            let buttons = params.get("buttons").and_then(Value::as_u64)
                .unwrap_or_else(|| mouse_button_mask(button));
            let (frame_id, x, y, target_node) = if matches!(event_type,
                "mouseMoved" | "mousePressed" | "mouseReleased" | "mouseWheel")
            {
                ctx.get_session_page_mut(session_id).ok_or("No page")?
                    .mouse_event_frame(event_type, x, y)?
            } else { (0, x, y, None) };
            let payload = json!({
                "kind": "mouse", "type": event_type, "x": x, "y": y,
                "nodeId": target_node, "buttonCode": mouse_button_code(button),
                "buttons": buttons, "clickCount": params.get("clickCount").and_then(Value::as_u64).unwrap_or(1),
                "modifiers": params.get("modifiers").and_then(Value::as_u64).unwrap_or(0),
                "deltaX": params.get("deltaX").and_then(Value::as_f64).unwrap_or(0.0),
                "deltaY": params.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0),
            });
            let moved_frame = {
                let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
                page.dispatch_native_input_in_frame(frame_id, &payload)?;
                (event_type == "mouseReleased" && frame_id == 0 && !page.has_pending_navigation()
                    && page.sync_virtual_url())
                    .then(|| (page.frame_id.clone(), page.url_string()))
            };
            if let Some((frame_id, url)) = moved_frame {
                crate::domains::page::emit_same_document_navigation(ctx, session_id, &frame_id, &url, "historyApi");
            }
            Ok(json!({}))
        }
        "insertText" => {
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            page.dispatch_native_input(&json!({ "kind": "insertText", "text": text }))?;
            Ok(json!({}))
        }
        "dispatchKeyEvent" => {
            let event_type = params.get("type").and_then(Value::as_str).unwrap_or("");
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            let payload = json!({
                "kind": "key", "type": event_type,
                "key": params.get("key").and_then(Value::as_str).unwrap_or(""),
                "code": params.get("code").and_then(Value::as_str).unwrap_or(""),
                "text": text,
                "windowsVirtualKeyCode": params.get("windowsVirtualKeyCode").and_then(Value::as_u64).unwrap_or(0),
                "modifiers": params.get("modifiers").and_then(Value::as_u64).unwrap_or(0),
            });
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            page.dispatch_native_input(&payload)?;
            if event_type == "char" && !text.is_empty() {
                // Preserve the existing event-loop turn for framework observers.
                page.settle(50).await;
            }
            Ok(json!({}))
        }
        "dispatchTouchEvent" => Err("Input.dispatchTouchEvent is unsupported".into()),
        "setIgnoreInputEvents" => Ok(json!({})),
        _ => Err(format!("Unknown Input method: {method}")),
    }
}

