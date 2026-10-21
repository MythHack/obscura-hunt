use std::collections::HashMap;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{WebSocketStream, tungstenite::handshake::derive_accept_key};
use url::Url;

use crate::{ObscuraNetError, SameSiteContext, same_site};

pub use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
pub use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};

trait SocketIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> SocketIo for T {}

/// An already authenticated HTTP Upgrade on the page's selected transport.
/// Framing never opens another connection, resolves another host or bypasses a proxy.
pub struct WebSocketConnection {
    stream: WebSocketStream<Box<dyn SocketIo>>,
    pub protocol: String,
}

impl WebSocketConnection {
    pub(crate) async fn upgraded<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        io: T, protocol: String,
    ) -> Self {
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .read_buffer_size(16 * 1024).write_buffer_size(16 * 1024)
            .max_write_buffer_size(65 * 1024 * 1024);
        let io: Box<dyn SocketIo> = Box::new(io);
        Self { stream: WebSocketStream::from_raw_socket(io,
            tokio_tungstenite::tungstenite::protocol::Role::Client, Some(config)).await, protocol }
    }

    pub async fn receive(&mut self) -> Option<Result<Message, WebSocketError>> {
        futures_util::StreamExt::next(&mut self.stream).await
    }

    pub async fn send(&mut self, message: Message) -> Result<(), WebSocketError> {
        futures_util::SinkExt::send(&mut self.stream, message).await
    }

    pub async fn flush(&mut self) -> Result<(), WebSocketError> {
        futures_util::SinkExt::flush(&mut self.stream).await
    }
}

/// Native document authority, not page-supplied handshake headers.
pub struct WebSocketRequest {
    pub url: Url,
    pub origin: String,
    pub protocols: Vec<String>,
}

impl WebSocketRequest {
    pub(crate) fn http_url(&self) -> Result<Url, ObscuraNetError> {
        let mut url = self.url.clone();
        let scheme = match url.scheme() {
            "ws" => "http", "wss" => "https",
            _ => return Err(network_error("Invalid WebSocket scheme")),
        };
        if url.fragment().is_some() {
            return Err(network_error("Invalid WebSocket URL"));
        }
        url.set_scheme(scheme).map_err(|_| network_error("Invalid WebSocket URL"))?;
        Ok(url)
    }

    pub(crate) fn cookie_context(&self, target: &Url) -> SameSiteContext {
        if Url::parse(&self.origin).ok().is_some_and(|origin| same_site(&origin, target)) {
            SameSiteContext::SameSite
        } else { SameSiteContext::CrossSite }
    }

    pub(crate) fn headers(&self, extra: &HashMap<String, String>, target: &Url) -> (String, Vec<(String, String)>) {
        let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
        let mut headers = Vec::with_capacity(extra.len() + 8 + usize::from(!self.protocols.is_empty()));
        for (name, value) in extra {
            let lower = name.to_ascii_lowercase();
            if matches!(lower.as_str(), "origin" | "host" | "connection"
                | "upgrade" | "cookie" | "content-length" | "transfer-encoding")
                || lower.starts_with("sec-websocket-") || lower.starts_with("sec-fetch-") { continue; }
            headers.push((name.clone(), value.clone()));
        }
        headers.extend([
            ("origin".into(), self.origin.clone()),
            ("connection".into(), "Upgrade".into()),
            ("upgrade".into(), "websocket".into()),
            ("sec-websocket-version".into(), "13".into()),
            ("sec-websocket-key".into(), key.clone()),
            ("sec-fetch-mode".into(), "websocket".into()),
            ("sec-fetch-dest".into(), "empty".into()),
        ]);
        let site = Url::parse(&self.origin).ok().map(|origin| {
            let request = crate::ResourceRequest::subresource(crate::ResourceType::Other, &origin);
            crate::client::request_fetch_site(&request, target)
        }).unwrap_or("cross-site");
        headers.push(("sec-fetch-site".into(), site.into()));
        if !self.protocols.is_empty() {
            headers.push(("sec-websocket-protocol".into(), self.protocols.join(", ")));
        }
        (key, headers)
    }

    /// Reject unsolicited extensions (none are offered) and ambiguous/invalid
    /// subprotocols before handing any server frame to the application.
    pub(crate) fn validate_response<'a>(
        &self, status: u16, key: &str, headers: impl Iterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<String, ObscuraNetError> {
        let mut upgrade = false;
        let mut connection = false;
        let mut accept = None;
        let mut protocol = None;
        for (name, value) in headers {
            if !matches!(name, "upgrade" | "connection" | "sec-websocket-accept"
                | "sec-websocket-protocol" | "sec-websocket-extensions") { continue; }
            let value = std::str::from_utf8(value).map_err(|_| network_error("Invalid Upgrade header"))?
                .trim_matches([' ', '\t']);
            match name {
                "upgrade" => upgrade |= value.split(',').any(|token| token.trim_matches([' ', '\t']).eq_ignore_ascii_case("websocket")),
                "sec-websocket-accept" if accept.is_none() => accept = Some(value),
                "sec-websocket-protocol" if protocol.is_none() => protocol = Some(value),
                "connection" => connection |= value.split(',').any(|token| token.trim_matches([' ', '\t']).eq_ignore_ascii_case("upgrade")),
                "sec-websocket-extensions" | "sec-websocket-accept" | "sec-websocket-protocol" =>
                    return Err(network_error("Unsolicited or duplicate WebSocket handshake header")),
                _ => {},
            }
        }
        if status != 101 || !upgrade || !connection
            || accept != Some(derive_accept_key(key.as_bytes()).as_str()) {
            return Err(network_error("WebSocket Upgrade was rejected"));
        }
        match protocol {
            Some(protocol) if self.protocols.iter().any(|candidate| candidate == protocol) => Ok(protocol.into()),
            None if self.protocols.is_empty() => Ok(String::new()),
            _ => Err(network_error("Invalid WebSocket subprotocol response")),
        }
    }
}

pub(crate) fn network_error(message: impl Into<String>) -> ObscuraNetError {
    ObscuraNetError::Network(message.into())
}
