//! WebRTC's reactor seam enforces the page's actual route for every packet.
//! SOCKS5 UDP uses RFC 1928 association framing; TURN/TCP uses RFC 8656
//! stream framing over authenticated SOCKS5/HTTP CONNECT, not a direct fallback.
use std::collections::HashMap;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Duration;
use base64::Engine;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use webrtc::runtime::{AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime, TokioRuntime, Transmit};
use obscura_net::ObscuraHttpClient;
#[cfg(feature = "stealth")]
use obscura_net::StealthHttpClient;

fn denied(message: &str) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, message) }
fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
type IoFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ServerTransport { Udp, Tcp, Tls(String) }
trait PacketStream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> PacketStream for T {}
type Stream = Box<dyn PacketStream>;
#[derive(Clone)]
pub(super) struct Route {
    pub http: Arc<ObscuraHttpClient>,
    #[cfg(feature = "stealth")]
    pub stealth: Option<Arc<StealthHttpClient>>,
    pub proxy: Option<url::Url>,
    pub servers: Arc<Mutex<HashMap<SocketAddr, ServerTransport>>>,
    pub events: Option<tokio::sync::mpsc::Sender<serde_json::Value>>,
    pub reactor: Arc<Mutex<Option<tokio::runtime::Handle>>>,
}
impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("RtcRoute").field("proxied", &self.proxy.is_some()).finish() }
}
impl Route {
    fn report_error(&self, error: &io::Error) {
        if let Some(events) = &self.events {
            let _ = events.try_send(serde_json::json!({"type":"icecandidateerror","errorCode":701,"errorText":error.to_string()}));
        }
    }
    pub fn allowed(&self, ip: IpAddr) -> bool {
        #[cfg(feature = "stealth")]
        if let Some(client) = &self.stealth { return obscura_ssrf::address_allowed(ip, client.allow_private_network, &client.allow_network); }
        obscura_ssrf::address_allowed(ip, self.http.allow_private_network, &self.http.allow_network)
    }
    pub fn check(&self, address: SocketAddr) -> io::Result<()> {
        if self.allowed(address.ip()) { Ok(()) } else { Err(denied("RTC destination is blocked by network policy")) }
    }
    pub async fn resolve(&self, host: String, port: u16) -> io::Result<Vec<SocketAddr>> {
        crate::rtc_dns::resolve_rtc_host(self.http.clone(), #[cfg(feature = "stealth")] self.stealth.clone(), host, port).await
    }
    async fn proxy_stream(&self, target: SocketAddr, udp: bool) -> io::Result<(TcpStream, SocketAddr)> {
        let result = self.open_proxy_stream(target, udp).await;
        if let Err(error) = &result { self.report_error(error); }
        result
    }
    async fn open_proxy_stream(&self, target: SocketAddr, udp: bool) -> io::Result<(TcpStream, SocketAddr)> {
        let proxy = self.proxy.as_ref().ok_or_else(|| invalid("Missing RTC proxy route"))?;
        // Resolving the operator's proxy host is the only ambient DNS lookup.
        // Destination names are resolved via the selected routed DoH client.
        let host = proxy.host_str().ok_or_else(|| invalid("Invalid RTC proxy host"))?;
        let port = proxy.port_or_known_default().or_else(|| matches!(proxy.scheme(), "socks5" | "socks5h").then_some(1080))
            .ok_or_else(|| invalid("RTC proxy port is required"))?;
        let mut stream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect((host, port))).await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RTC proxy connection timed out"))??;
        stream.set_nodelay(true)?;
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            match proxy.scheme() {
                "socks5" | "socks5h" => {
                    let user = percent_decode(proxy.username())?;
                    let password = percent_decode(proxy.password().unwrap_or(""))?;
                    let authenticated = !user.is_empty() || !password.is_empty();
                    stream.write_all(if authenticated { &[5, 1, 2] } else { &[5, 1, 0] }).await?;
                    let mut reply = [0; 2]; stream.read_exact(&mut reply).await?;
                    if reply != [5, if authenticated { 2 } else { 0 }] { return Err(denied("RTC SOCKS5 authentication method rejected")); }
                    if authenticated {
                        if user.len() > 255 || password.len() > 255 { return Err(invalid("RTC SOCKS5 credential is too long")); }
                        let mut credentials = Vec::with_capacity(3 + user.len() + password.len());
                        credentials.extend_from_slice(&[1, user.len() as u8]); credentials.extend_from_slice(&user);
                        credentials.push(password.len() as u8); credentials.extend_from_slice(&password);
                        stream.write_all(&credentials).await?; stream.read_exact(&mut reply).await?;
                        if reply != [1, 0] { return Err(denied("RTC SOCKS5 authentication rejected")); }
                    }
                    let mut request = Vec::with_capacity(22); request.extend_from_slice(&[5, if udp { 3 } else { 1 }, 0]);
                    put_address(&mut request, target); stream.write_all(&request).await?;
                    let mut header = [0; 4]; stream.read_exact(&mut header).await?;
                    if header[..3] != [5, 0, 0] { return Err(denied("RTC SOCKS5 route rejected")); }
                    let bound = read_address(&mut stream, header[3]).await?;
                    Ok(bound)
                }
                "http" if !udp => {
                    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
                    if !proxy.username().is_empty() || proxy.password().is_some() {
                        let mut credentials = percent_decode(proxy.username())?; credentials.push(b':');
                        credentials.extend_from_slice(&percent_decode(proxy.password().unwrap_or(""))?);
                        request.push_str("Proxy-Authorization: Basic ");
                        request.push_str(&base64::engine::general_purpose::STANDARD.encode(credentials)); request.push_str("\r\n");
                    }
                    request.push_str("\r\n"); stream.write_all(request.as_bytes()).await?;
                    let mut response = Vec::with_capacity(512);
                    while !response.ends_with(b"\r\n\r\n") {
                        if response.len() == 16384 { return Err(invalid("RTC CONNECT response exceeds limit")); }
                        response.push(stream.read_u8().await?);
                    }
                    let status = std::str::from_utf8(&response).ok().and_then(|s| s.lines().next()).and_then(|s| s.split_ascii_whitespace().nth(1));
                    if status != Some("200") { return Err(denied("RTC HTTP CONNECT route rejected")); }
                    Ok(stream.local_addr()?)
                }
                _ => Err(io::Error::new(io::ErrorKind::Unsupported, "RTC route requires SOCKS5 UDP or TURN/TCP over SOCKS5/HTTP CONNECT")),
            }
        }).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RTC proxy negotiation timed out"))??;
        Ok((stream, result))
    }
    async fn turn_tcp(&self, target: SocketAddr) -> io::Result<Stream> {
        let result = self.open_turn_tcp(target).await;
        if let Err(error) = &result { self.report_error(error); }
        result
    }
    async fn open_turn_tcp(&self, target: SocketAddr) -> io::Result<Stream> {
        self.check(target)?;
        let stream = if self.proxy.is_some() { self.proxy_stream(target, false).await?.0 }
            else { tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(target)).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RTC TCP connection timed out"))?? };
        let transport = self.servers.lock().map_err(|_| invalid("RTC route lock poisoned"))?.get(&target).cloned();
        if let Some(ServerTransport::Tls(host)) = transport {
            let connector = tokio_rustls::TlsConnector::from(tls_configuration()?);
            let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host).map_err(|_| invalid("Invalid ICE TLS server name"))?;
            let secured = tokio::time::timeout(Duration::from_secs(15), connector.connect(name, stream)).await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ICE TLS handshake timed out"))??;
            Ok(Box::new(secured))
        } else { Ok(Box::new(stream)) }
    }
}
fn tls_configuration() -> io::Result<Arc<tokio_rustls::rustls::ClientConfig>> {
    use tokio_rustls::rustls;
    static CONFIG: std::sync::LazyLock<Result<Arc<rustls::ClientConfig>, String>> = std::sync::LazyLock::new(|| {
        let loaded = rustls_native_certs::load_native_certs();
        if loaded.certs.is_empty() { return Err("ICE TLS has no trusted native or explicitly configured certificates".into()); }
        let mut roots = rustls::RootCertStore::empty();
        for certificate in loaded.certs { roots.add(certificate).map_err(|error| error.to_string())?; }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?.with_root_certificates(roots).with_no_client_auth();
        Ok(Arc::new(config))
    });
    CONFIG.clone().map_err(io::Error::other)
}
fn percent_decode(value: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(value.len()); let input = value.as_bytes(); let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let hex = input.get(i + 1..i + 3).and_then(|s| std::str::from_utf8(s).ok()).and_then(|s| u8::from_str_radix(s, 16).ok()).ok_or_else(|| invalid("Invalid proxy credential encoding"))?;
            bytes.push(hex); i += 3;
        } else { bytes.push(input[i]); i += 1; }
    }
    Ok(bytes)
}
fn put_address(buffer: &mut Vec<u8>, target: SocketAddr) {
    match target.ip() { IpAddr::V4(ip) => { buffer.push(1); buffer.extend_from_slice(&ip.octets()); }, IpAddr::V6(ip) => { buffer.push(4); buffer.extend_from_slice(&ip.octets()); } }
    buffer.extend_from_slice(&target.port().to_be_bytes());
}
async fn read_address(stream: &mut TcpStream, kind: u8) -> io::Result<SocketAddr> {
    let ip = match kind {
        1 => { let mut ip = [0; 4]; stream.read_exact(&mut ip).await?; IpAddr::from(ip) },
        4 => { let mut ip = [0; 16]; stream.read_exact(&mut ip).await?; IpAddr::from(ip) },
        // A proxy's UDP relay must provide an address we can dial without ambient
        // destination DNS. RFC1928 permits a domain; fail rather than leak it.
        _ => return Err(invalid("SOCKS5 proxy returned a non-address relay endpoint")),
    };
    Ok(SocketAddr::new(ip, stream.read_u16().await?))
}
fn decode_address(bytes: &[u8]) -> io::Result<(SocketAddr, usize)> {
    let (ip, end) = match bytes.first() {
        Some(1) if bytes.len() >= 7 => (IpAddr::from(<[u8; 4]>::try_from(&bytes[1..5]).map_err(|_| invalid("Invalid SOCKS datagram"))?), 5),
        Some(4) if bytes.len() >= 19 => (IpAddr::from(<[u8; 16]>::try_from(&bytes[1..17]).map_err(|_| invalid("Invalid SOCKS datagram"))?), 17),
        _ => return Err(invalid("Invalid SOCKS5 UDP endpoint")),
    };
    Ok((SocketAddr::new(ip, u16::from_be_bytes([bytes[end], bytes[end + 1]])), end + 2))
}

impl Runtime for Route {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        let reactor = self.reactor.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().expect("RTC reactor is captured before native stack creation");
        let _entered = reactor.enter(); TokioRuntime.spawn(future)
    }
    fn spawn_reactor(&self, size: usize, future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        let reactor = self.reactor.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().expect("RTC reactor is captured before native stack creation");
        let _entered = reactor.enter(); TokioRuntime.spawn_reactor(size, future)
    }
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        socket.set_nonblocking(true)?;
        let local = socket.local_addr()?;
        let udp = Arc::new(UdpSocket::from_std(socket)?);
        let association = if self.proxy.is_some() {
            let route = self.clone(); let udp = udp.clone();
            Some(Box::pin(async move {
                let request = SocketAddr::new(if local.is_ipv4() { IpAddr::from([0, 0, 0, 0]) } else { IpAddr::from([0u16; 8]) }, 0);
                let (control, mut relay) = route.proxy_stream(request, true).await?;
                let proxy_address = control.peer_addr()?.ip();
                if relay.ip().is_unspecified() { relay.set_ip(proxy_address); }
                if relay.ip() != proxy_address { route.check(relay)?; }
                udp.connect(relay).await?;
                Ok(Association { control, relay })
            }) as IoFuture<Association>)
        } else { None };
        Ok(Arc::new(PacketSocket { route: self.clone(), udp, local,
            state: Mutex::new(PacketState { association, ready: None, failure: None, tcp: HashMap::new(), send: Vec::with_capacity(65536), recv: vec![0; 65536] }) }))
    }
    fn wrap_tcp_listener(&self, _listener: std::net::TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Inbound ICE-TCP is not enabled for data-channel connections"))
    }
    fn connect_tcp<'a>(&'a self, remote: SocketAddr) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        Box::pin(async move {
            self.check(remote)?;
            if self.proxy.is_some() { return Err(denied("Direct peer TCP is forbidden on a relay-only RTC route")); }
            TokioRuntime.connect_tcp(remote).await
        })
    }
    fn resolve_host<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            if let Ok(address) = host.parse::<SocketAddr>() { self.check(address)?; return Ok(vec![address]); }
            let (host, port) = host.rsplit_once(':').ok_or_else(|| invalid("Missing RTC destination port"))?;
            self.resolve(host.trim_matches(['[', ']']).to_owned(), port.parse().map_err(|_| invalid("Invalid RTC destination port"))?).await
        })
    }
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> { TokioRuntime.sleep(duration) }
    fn interval(&self, duration: Duration) -> Box<dyn AsyncInterval> { TokioRuntime.interval(duration) }
    fn block_on(&self, future: Pin<Box<dyn Future<Output = ()> + '_>>) { TokioRuntime.block_on(future) }
    fn name(&self) -> &'static str { "obscura-policy-tokio" }
}
struct Association { control: TcpStream, relay: SocketAddr }
struct PacketState {
    association: Option<IoFuture<Association>>, ready: Option<Association>,
    failure: Option<(io::ErrorKind, String)>,
    tcp: HashMap<SocketAddr, TcpPacketPipe>, send: Vec<u8>, recv: Vec<u8>,
}
struct PacketSocket { route: Route, udp: Arc<UdpSocket>, local: SocketAddr, state: Mutex<PacketState> }
impl std::fmt::Debug for PacketSocket { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("RtcPacketSocket").field("proxied", &self.route.proxy.is_some()).finish() } }
impl PacketState {
    fn associate(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some((kind, message)) = &self.failure { return Poll::Ready(Err(io::Error::new(*kind, message.clone()))); }
        if let Some(future) = self.association.as_mut() {
            let result = ready!(future.as_mut().poll(cx)); self.association = None;
            match result {
                Ok(association) => self.ready = Some(association),
                Err(error) => { self.failure = Some((error.kind(), error.to_string())); return Poll::Ready(Err(error)); },
            }
        }
        if let Some(association) = &self.ready {
            match association.control.try_read(&mut [0; 1]) {
                Ok(_) => return Poll::Ready(Err(denied("SOCKS5 UDP control connection closed"))),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {},
                Err(error) => return Poll::Ready(Err(error)),
            }
            if let Poll::Ready(Err(error)) = association.control.poll_read_ready(cx) { return Poll::Ready(Err(error)); }
        }
        Poll::Ready(Ok(()))
    }
}
impl AsyncUdpSocket for PacketSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> { Ok(self.local) }
    fn poll_send(&self, cx: &mut Context<'_>, transmit: &Transmit<'_>) -> Poll<io::Result<usize>> {
        self.route.check(transmit.destination)?;
        let stream_transport = self.route.servers.lock().map_err(|_| invalid("RTC route lock poisoned"))?.get(&transmit.destination)
            .map(|transport| matches!(transport, ServerTransport::Tcp | ServerTransport::Tls(_)));
        if self.route.proxy.is_some() && stream_transport.is_none() { return Poll::Ready(Err(denied("RTC proxy route permits configured TURN servers only"))); }
        let mut state = self.state.lock().map_err(|_| invalid("RTC socket lock poisoned"))?;
        if stream_transport == Some(true) {
            let pipe = state.tcp.entry(transmit.destination).or_insert_with(|| TcpPacketPipe::new(self.route.clone(), transmit.destination));
            return pipe.send(cx, transmit.contents);
        }
        if self.route.proxy.is_none() { return self.udp.poll_send_to(cx, transmit.contents, transmit.destination); }
        ready!(state.associate(cx))?;
        state.send.clear(); state.send.extend_from_slice(&[0, 0, 0]); put_address(&mut state.send, transmit.destination); state.send.extend_from_slice(transmit.contents);
        let relay = state.ready.as_ref().ok_or_else(|| invalid("Missing SOCKS5 UDP association"))?.relay;
        ready!(self.udp.poll_send_to(cx, &state.send, relay))?;
        Poll::Ready(Ok(transmit.contents.len()))
    }
    fn poll_recv(&self, cx: &mut Context<'_>, buffers: &mut [IoSliceMut<'_>], meta: &mut [RecvMeta]) -> Poll<io::Result<usize>> {
        if buffers.is_empty() || meta.is_empty() { return Poll::Ready(Ok(0)); }
        let mut state = self.state.lock().map_err(|_| invalid("RTC socket lock poisoned"))?;
        for (target, pipe) in &mut state.tcp {
            match pipe.recv(cx, &mut buffers[0]) {
                Poll::Ready(Ok(length)) => { fill_meta(&mut meta[0], *target, length); return Poll::Ready(Ok(1)); },
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)), Poll::Pending => {},
            }
        }
        if self.route.proxy.is_some() && !self.route.servers.lock().map_err(|_| invalid("RTC route lock poisoned"))?.values().any(|t| *t == ServerTransport::Udp) { return Poll::Pending; }
        ready!(state.associate(cx))?;
        let expected_relay = state.ready.as_ref().map(|association| association.relay);
        let mut read = tokio::io::ReadBuf::new(&mut state.recv);
        let source = ready!(self.udp.poll_recv_from(cx, &mut read))?;
        let bytes = read.filled();
        let (source, payload) = if self.route.proxy.is_some() {
            if expected_relay != Some(source) || bytes.get(..3) != Some(&[0, 0, 0]) { return Poll::Ready(Err(invalid("Invalid SOCKS5 UDP datagram"))); }
            let (source, offset) = decode_address(&bytes[3..])?; (source, &bytes[3 + offset..])
        } else { (source, bytes) };
        self.route.check(source)?;
        if payload.len() > buffers[0].len() { return Poll::Ready(Err(invalid("RTC datagram exceeds receive buffer"))); }
        buffers[0][..payload.len()].copy_from_slice(payload); fill_meta(&mut meta[0], source, payload.len());
        Poll::Ready(Ok(1))
    }
}
fn fill_meta(meta: &mut RecvMeta, address: SocketAddr, len: usize) { *meta = RecvMeta::default(); meta.addr = address; meta.len = len; meta.stride = len.max(1); }

// TURN over TCP carries complete STUN or ChannelData frames, without RFC4571's
// extra length prefix. Padding is required for ChannelData on stream transport.
struct TcpPacketPipe { connecting: Option<IoFuture<Stream>>, stream: Option<Stream>, failure: Option<(io::ErrorKind, String)>, read: Vec<u8>, read_start: usize, read_end: usize, written: usize, write: Vec<u8> }
impl TcpPacketPipe {
    fn new(route: Route, target: SocketAddr) -> Self {
        Self { connecting: Some(Box::pin(async move { route.turn_tcp(target).await })), stream: None, failure: None, read: vec![0; 65556], read_start: 0, read_end: 0, written: 0, write: Vec::with_capacity(65556) }
    }
    fn connected(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some((kind, message)) = &self.failure { return Poll::Ready(Err(io::Error::new(*kind, message.clone()))); }
        if let Some(future) = self.connecting.as_mut() {
            let result = ready!(future.as_mut().poll(cx)); self.connecting = None;
            match result {
                Ok(stream) => self.stream = Some(stream),
                Err(error) => { self.failure = Some((error.kind(), error.to_string())); return Poll::Ready(Err(error)); },
            }
        }
        Poll::Ready(Ok(()))
    }
    fn send(&mut self, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        ready!(self.connected(cx))?;
        if self.write.is_empty() {
            self.write.extend_from_slice(bytes);
            if bytes.first().is_some_and(|b| b & 0xc0 == 0x40) { self.write.resize((bytes.len() + 3) & !3, 0); }
        }
        let stream = self.stream.as_mut().ok_or_else(|| invalid("Missing TURN TCP stream"))?;
        while self.written < self.write.len() {
            match ready!(Pin::new(&mut **stream).poll_write(cx, &self.write[self.written..])) {
                Ok(0) => return Poll::Ready(Err(io::Error::new(io::ErrorKind::WriteZero, "TURN TCP stream closed"))),
                Ok(n) => self.written += n,
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
        ready!(Pin::new(&mut **stream).poll_flush(cx))?;
        self.write.clear(); self.written = 0; Poll::Ready(Ok(bytes.len()))
    }
    fn recv(&mut self, cx: &mut Context<'_>, buffer: &mut [u8]) -> Poll<io::Result<usize>> {
        ready!(self.connected(cx))?;
        loop {
            let available = &self.read[self.read_start..self.read_end];
            if available.len() >= 4 {
                let body = u16::from_be_bytes([available[2], available[3]]) as usize;
                let channel = available[0] & 0xc0 == 0x40;
                if !channel && available[0] & 0xc0 != 0 { return Poll::Ready(Err(invalid("Invalid TURN TCP frame"))); }
                let length = body + if channel { 4 } else { 20 };
                let padded = if channel { (length + 3) & !3 } else { length };
                if available.len() >= padded {
                    if length > buffer.len() { return Poll::Ready(Err(invalid("TURN TCP frame exceeds receive buffer"))); }
                    buffer[..length].copy_from_slice(&available[..length]); self.read_start += padded;
                    if self.read_start == self.read_end { self.read_start = 0; self.read_end = 0; }
                    return Poll::Ready(Ok(length));
                }
            }
            if self.read_end == self.read.len() && self.read_start != 0 {
                self.read.copy_within(self.read_start..self.read_end, 0); self.read_end -= self.read_start; self.read_start = 0;
            }
            if self.read_end == self.read.len() { return Poll::Ready(Err(invalid("TURN TCP frame exceeds protocol maximum"))); }
            let stream = self.stream.as_mut().ok_or_else(|| invalid("Missing TURN TCP stream"))?;
            let mut read = tokio::io::ReadBuf::new(&mut self.read[self.read_end..]);
            let result = Pin::new(&mut **stream).poll_read(cx, &mut read);
            self.read_end += read.filled().len();
            ready!(result)?;
            if read.filled().is_empty() { return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "TURN TCP stream closed"))); }
        }
    }
}
