//! Meeting server inside Kite (`kite://meet`).
//!
//! The host's Kite listens on a TCP port (opened on the home router with UPnP when it can) and
//! hands out an invite code: the host's addresses, a room id and a secret. A guest's Kite
//! connects there and knocks; the host's page shows it and the host accepts or refuses. After
//! that the server only relays WebRTC setup messages (offers, answers, ICE candidates) and
//! small chat/state messages between the members; audio and video go straight between the
//! members' browsers (P2P mesh), as before.
//!
//! Every frame is sealed with AES-256-GCM under a key derived from the invite secret, so only
//! people with the code can talk to the room, and nothing on the way can read or change the
//! setup (which carries the DTLS fingerprints that protect the media).
//!
//! Pages never touch the network themselves: they call the commands below and get `meet`
//! events (a `kite://` page may not open plain sockets to other computers anyway).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{Emitter, Manager, Webview};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;

const PREFIX: &str = "KITE-ROOM1.";
/// Preferred port (another one if taken).
const PORT: u16 = 47800;
const MAX_FRAME: usize = 512 * 1024;
const MAX_MEMBERS: usize = 8;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

// ---------------------------------------------------------------- invite and crypto

#[derive(Serialize, Deserialize)]
struct Invite {
    /// Room id (hex).
    r: String,
    /// Secret (base64url, 16 bytes).
    k: String,
    /// Where the host listens: public (router) first, then the local network.
    a: Vec<String>,
}

fn invite_code(inv: &Invite) -> String {
    format!("{PREFIX}{}", B64.encode(serde_json::to_vec(inv).unwrap_or_default()))
}

fn parse_invite(code: &str) -> Result<Invite, AppError> {
    let bad = || AppError::new("Kite 회의 초대 코드가 아닙니다 (KITE-ROOM1. 로 시작).", "bad invite");
    let raw: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let body = raw.strip_prefix(PREFIX).ok_or_else(bad)?;
    let inv: Invite = serde_json::from_slice(&B64.decode(body).map_err(|_| bad())?).map_err(|_| bad())?;
    if inv.a.is_empty() || B64.decode(&inv.k).map(|k| k.len()).unwrap_or(0) != 16 {
        return Err(bad());
    }
    Ok(inv)
}

/// Frame key: SHA-256 over a label, the secret and the room id.
struct Sealer(LessSafeKey);

impl Sealer {
    fn new(inv: &Invite) -> Result<Self, AppError> {
        let secret = B64.decode(&inv.k).map_err(|e| AppError::new("초대 코드가 손상되었습니다.", e))?;
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        ctx.update(b"kite-meet-v1\0");
        ctx.update(&secret);
        ctx.update(inv.r.as_bytes());
        let key = UnboundKey::new(&AES_256_GCM, ctx.finish().as_ref()).map_err(|_| AppError::new("암호 키를 만들지 못했습니다.", "key"))?;
        Ok(Self(LessSafeKey::new(key)))
    }

    fn seal(&self, v: &Value) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        let _ = SystemRandom::new().fill(&mut nonce);
        let mut buf = serde_json::to_vec(v).unwrap_or_default();
        if self.0.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf).is_err() {
            return Vec::new();
        }
        [nonce.as_slice(), &buf].concat()
    }

    fn open(&self, frame: &[u8]) -> Option<Value> {
        if frame.len() < NONCE_LEN {
            return None;
        }
        let (nonce, body) = frame.split_at(NONCE_LEN);
        let mut buf = body.to_vec();
        let plain = self.0.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::empty(), &mut buf).ok()?;
        serde_json::from_slice(plain).ok()
    }
}

async fn write_frame(w: &mut (impl AsyncWriteExt + Unpin), key: &Sealer, v: &Value) -> std::io::Result<()> {
    let f = key.seal(v);
    w.write_all(&(f.len() as u32).to_be_bytes()).await?;
    w.write_all(&f).await?;
    w.flush().await
}

/// `None`: closed, too big, or not sealed with the room key (someone without the code).
async fn read_frame(r: &mut (impl AsyncReadExt + Unpin), key: &Sealer) -> Option<Value> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.ok()?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await.ok()?;
    key.open(&buf)
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    let _ = SystemRandom::new().fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn clean_name(n: &str) -> String {
    let n: String = n.trim().chars().filter(|c| !c.is_control()).take(40).collect();
    if n.is_empty() { "이름 없음".into() } else { n }
}

// ---------------------------------------------------------------- sessions

struct Member {
    name: String,
    admitted: bool,
    tx: mpsc::UnboundedSender<Value>,
}

/// Where events for the meeting page go (the page's webview; a closure in tests).
type Sink = Arc<dyn Fn(Value) + Send + Sync>;

fn webview_sink(webview: &Webview) -> Sink {
    let (app, label) = (webview.app_handle().clone(), webview.label().to_string());
    Arc::new(move |v| {
        let _ = app.emit_to(label.as_str(), "meet", v);
    })
}

struct Host {
    sink: Sink,
    /// Meeting generation, sent with every event (the page drops ones of an older meeting).
    seq: u64,
    name: String,
    key: Arc<Sealer>,
    members: Mutex<HashMap<String, Member>>,
    stop: CancellationToken,
    /// Router port mapping to remove at the end.
    mapping: Mutex<Option<(Gateway, u16)>>,
}

struct Guest {
    tx: mpsc::UnboundedSender<Value>,
    stop: CancellationToken,
}

enum Session {
    Host(Arc<Host>),
    Guest(Arc<Guest>),
}

/// One meeting per meeting tab (by webview label): one tab can host while another tab of the
/// same Kite joins it, e.g. to try it alone.
static SESSIONS: Mutex<BTreeMap<String, Session>> = Mutex::new(BTreeMap::new());
/// Ties events to one meeting (a late event of an old one is dropped by the page).
static SEQ: AtomicU64 = AtomicU64::new(1);

impl Host {
    fn page(&self, v: Value) {
        (self.sink)(json!({ "seq": self.seq, "msg": v }));
    }

    /// Delivers `data` from `from` to one member (`to`), the host page (`"host"`), or everyone
    /// admitted but the sender (`None`).
    fn route(&self, from: &str, to: Option<&str>, data: Value) {
        let msg = json!({ "t": "from", "from": from, "data": data });
        let members = self.members.lock().unwrap();
        match to {
            Some("host") => self.page(msg),
            Some(id) => {
                if let Some(m) = members.get(id).filter(|m| m.admitted) {
                    let _ = m.tx.send(msg);
                }
            }
            None => {
                for (id, m) in members.iter() {
                    if m.admitted && id != from {
                        let _ = m.tx.send(msg.clone());
                    }
                }
                if from != "host" {
                    self.page(msg);
                }
            }
        }
    }

    fn admitted_list(&self, except: &str) -> Vec<Value> {
        let mut v = vec![json!({ "id": "host", "name": self.name })];
        v.extend(self.members.lock().unwrap().iter().filter(|(id, m)| m.admitted && id.as_str() != except).map(|(id, m)| json!({ "id": id, "name": m.name })));
        v
    }

    async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => break,
                r = listener.accept() => {
                    let Ok((stream, peer)) = r else { continue };
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(self.clone().member(stream, peer));
                }
            }
        }
    }

    /// One guest connection: knock, wait for the host, then relay.
    async fn member(self: Arc<Self>, stream: TcpStream, peer: SocketAddr) {
        let (mut rd, mut wr) = stream.into_split();
        // Must knock (sealed with the room key) within a few seconds.
        let knock = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut rd, &self.key)).await.ok().flatten();
        let Some(knock) = knock.filter(|k| k["t"] == "knock") else {
            tracing::debug!(%peer, "meeting: dropped a connection without a valid knock");
            return;
        };
        let name = clean_name(knock["name"].as_str().unwrap_or(""));
        let id = random_hex(4);
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        let full = {
            let mut members = self.members.lock().unwrap();
            let full = members.len() >= MAX_MEMBERS;
            if !full {
                members.insert(id.clone(), Member { name: name.clone(), admitted: false, tx });
            }
            full
        };
        if full {
            let _ = write_frame(&mut wr, &self.key, &json!({ "t": "deny", "reason": "full" })).await;
            return;
        }
        tracing::info!(%peer, %id, "meeting: knock");
        self.page(json!({ "t": "knock", "id": id, "name": name }));

        let key = self.key.clone();
        let stop = self.stop.clone();
        let writer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => {
                        let _ = write_frame(&mut wr, &key, &json!({ "t": "closed" })).await;
                        break;
                    }
                    m = rx.recv() => {
                        let Some(m) = m else { break };
                        let end = m["t"] == "deny";
                        if write_frame(&mut wr, &key, &m).await.is_err() || end {
                            break;
                        }
                    }
                }
            }
        });
        loop {
            let frame = tokio::select! {
                _ = self.stop.cancelled() => None,
                f = read_frame(&mut rd, &self.key) => f,
            };
            let Some(f) = frame else { break };
            let admitted = self.members.lock().unwrap().get(&id).is_some_and(|m| m.admitted);
            if !admitted {
                continue;
            }
            match f["t"].as_str() {
                Some("to") => self.route(&id, f["to"].as_str(), f["data"].clone()),
                Some("all") => self.route(&id, None, f["data"].clone()),
                _ => {}
            }
        }
        let was = self.members.lock().unwrap().remove(&id);
        writer.abort();
        if let Some(m) = was {
            if m.admitted {
                self.route_left(&id);
            } else {
                self.page(json!({ "t": "knock-gone", "id": id }));
            }
        }
        tracing::info!(%id, "meeting: member gone");
    }

    fn route_left(&self, id: &str) {
        let msg = json!({ "t": "left", "id": id });
        for m in self.members.lock().unwrap().values().filter(|m| m.admitted) {
            let _ = m.tx.send(msg.clone());
        }
        self.page(msg);
    }

    fn admit(&self, id: &str, accept: bool) {
        let mut members = self.members.lock().unwrap();
        let Some(m) = members.get_mut(id) else { return };
        if !accept {
            let _ = m.tx.send(json!({ "t": "deny" }));
            return;
        }
        m.admitted = true;
        let name = m.name.clone();
        let tx = m.tx.clone();
        drop(members);
        let _ = tx.send(json!({ "t": "admit", "you": id, "peers": self.admitted_list(id) }));
        let joined = json!({ "t": "joined", "id": id, "name": name });
        for (mid, m) in self.members.lock().unwrap().iter() {
            if m.admitted && mid != id {
                let _ = m.tx.send(joined.clone());
            }
        }
        self.page(joined);
    }
}

fn stop_session(label: &str) {
    let old = SESSIONS.lock().unwrap().remove(label);
    match old {
        Some(Session::Host(h)) => {
            h.stop.cancel();
            if let Some((gw, port)) = h.mapping.lock().unwrap().take() {
                tauri::async_runtime::spawn(async move {
                    let _ = gw.remove_port(igd_next::PortMappingProtocol::TCP, port).await;
                });
            }
        }
        Some(Session::Guest(g)) => g.stop.cancel(),
        None => {}
    }
}

/// The address this computer uses towards the internet (no packet is sent).
fn local_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

type Gateway = igd_next::aio::Gateway<igd_next::aio::tokio::Tokio>;

/// What the router could do for people outside.
enum Router {
    /// Port opened: reachable at this public address.
    Open(Gateway, u16, IpAddr),
    /// The router's own outside address is private (it sits behind another router or the
    /// provider's NAT): a port opened here would not reach the internet.
    DoubleNat,
    /// No UPnP (or it refused).
    None,
}

/// Not reachable from the internet: private, shared (CGNAT 100.64/10), link-local, loopback.
fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
        IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Opens `port` on the router (UPnP) when that makes this computer reachable from the internet.
async fn upnp(local: SocketAddr) -> Router {
    let mut opts = igd_next::SearchOptions::default();
    opts.timeout = Some(Duration::from_secs(3));
    opts.single_search_timeout = Some(Duration::from_secs(2));
    let gw = match igd_next::aio::tokio::search_gateway(opts).await {
        Ok(g) => g,
        Err(e) => {
            tracing::info!(error = %e, "meeting: no UPnP router");
            return Router::None;
        }
    };
    let Ok(ip) = gw.get_external_ip().await else { return Router::None };
    if is_internal(ip) {
        tracing::info!("meeting: router is behind another NAT (its outside address is private)");
        return Router::DoubleNat;
    }
    for ext in [local.port(), local.port() + 1, local.port() + 2, local.port() + 11] {
        match gw.add_port(igd_next::PortMappingProtocol::TCP, ext, local, 4 * 3600, "Kite 영상회의").await {
            Ok(()) => {
                tracing::info!(ext, "meeting: router port opened (UPnP)");
                return Router::Open(gw, ext, ip);
            }
            Err(e) => tracing::info!(error = %e, ext, "meeting: UPnP mapping refused"),
        }
    }
    Router::None
}

// ---------------------------------------------------------------- commands

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    code: String,
    seq: u64,
    port: u16,
    /// Public address when the router port was opened, so people elsewhere can join.
    public: Option<String>,
    lan: Option<String>,
    /// Why people elsewhere cannot join: `double-nat`, `no-upnp` (None when they can).
    blocked: Option<&'static str>,
}

/// Starts a meeting hosted by this Kite; returns the invite code.
#[tauri::command]
pub async fn meet_host(webview: Webview, name: String) -> Result<HostInfo, AppError> {
    stop_session(webview.label());
    let (host, info) = host_start(webview_sink(&webview), &name, true).await?;
    SESSIONS.lock().unwrap().insert(webview.label().to_string(), Session::Host(host));
    Ok(info)
}

/// `upnp`: try to open the router port (off in tests).
async fn host_start(sink: Sink, name: &str, try_upnp: bool) -> Result<(Arc<Host>, HostInfo), AppError> {
    let listener = match TcpListener::bind(("0.0.0.0", PORT)).await {
        Ok(l) => l,
        Err(_) => TcpListener::bind(("0.0.0.0", 0)).await.map_err(|e| AppError::new("회의 서버를 열지 못했습니다.", e))?,
    };
    let port = listener.local_addr().map_err(|e| AppError::new("회의 서버를 열지 못했습니다.", e))?.port();
    let lan_ip = local_ip();
    let router = match lan_ip {
        Some(ip) if try_upnp => upnp(SocketAddr::new(ip, port)).await,
        _ => Router::None,
    };
    let (mapping, blocked) = match router {
        Router::Open(gw, ext, ip) => (Some((gw, ext, ip)), None),
        Router::DoubleNat => (None, Some("double-nat")),
        Router::None => (None, Some("no-upnp")),
    };
    let mut secret = [0u8; 16];
    let _ = SystemRandom::new().fill(&mut secret);
    let public = mapping.as_ref().map(|(_, ext, ip)| SocketAddr::new(*ip, *ext).to_string());
    let lan = lan_ip.map(|ip| SocketAddr::new(ip, port).to_string());
    let inv = Invite { r: random_hex(6), k: B64.encode(secret), a: public.iter().chain(lan.iter()).cloned().collect() };
    if inv.a.is_empty() {
        return Err(AppError::new("이 컴퓨터의 네트워크 주소를 찾지 못했습니다. 네트워크 연결을 확인하세요.", "no address"));
    }
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let host = Arc::new(Host {
        sink,
        seq,
        name: clean_name(name),
        key: Arc::new(Sealer::new(&inv)?),
        members: Mutex::new(HashMap::new()),
        stop: CancellationToken::new(),
        mapping: Mutex::new(mapping.map(|(gw, ext, _)| (gw, ext))),
    });
    tokio::spawn(host.clone().serve(listener));
    tracing::info!(port, public = ?public, lan = ?lan, "meeting: hosting");
    Ok((host, HostInfo { code: invite_code(&inv), seq, port, public, lan, blocked }))
}

/// Joins a meeting with an invite code: connects to the host and knocks.
#[tauri::command]
pub async fn meet_join(webview: Webview, code: String, name: String) -> Result<u64, AppError> {
    parse_invite(&code)?;
    stop_session(webview.label());
    let (guest, seq) = join_start(webview_sink(&webview), &code, &name).await?;
    SESSIONS.lock().unwrap().insert(webview.label().to_string(), Session::Guest(guest));
    Ok(seq)
}

async fn join_start(sink: Sink, code: &str, name: &str) -> Result<(Arc<Guest>, u64), AppError> {
    let inv = parse_invite(code)?;
    let key = Arc::new(Sealer::new(&inv)?);
    // Every address at once; the first that answers wins.
    let (found_tx, mut found_rx) = mpsc::channel::<TcpStream>(1);
    for a in inv.a.iter().filter_map(|a| a.parse::<SocketAddr>().ok()) {
        let tx = found_tx.clone();
        tokio::spawn(async move {
            if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(8), TcpStream::connect(a)).await {
                let _ = tx.try_send(s);
            }
        });
    }
    drop(found_tx);
    let stream = found_rx.recv().await.ok_or_else(|| {
        AppError::new("회의를 연 컴퓨터에 연결하지 못했습니다. 회의가 아직 열려 있는지, 그리고 다른 네트워크라면 그쪽 공유기에서 외부 접속(UPnP)이 되는지 확인하세요.", "unreachable")
    })?;
    let _ = stream.set_nodelay(true);
    let (mut rd, mut wr) = stream.into_split();
    write_frame(&mut wr, &key, &json!({ "t": "knock", "name": clean_name(name) })).await.map_err(|e| AppError::new("회의 서버에 요청을 보내지 못했습니다.", e))?;

    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let guest = Arc::new(Guest { tx, stop: CancellationToken::new() });
    let stop = guest.stop.clone();
    let wkey = key.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                m = rx.recv() => {
                    let Some(m) = m else { break };
                    if write_frame(&mut wr, &wkey, &m).await.is_err() { break; }
                }
            }
        }
    });
    let stop = guest.stop.clone();
    tokio::spawn(async move {
        loop {
            let f = tokio::select! {
                _ = stop.cancelled() => return,
                f = read_frame(&mut rd, &key) => f,
            };
            let Some(f) = f else { break };
            sink(json!({ "seq": seq, "msg": f }));
        }
        sink(json!({ "seq": seq, "msg": { "t": "closed" } }));
    });
    Ok((guest, seq))
}

/// The tab is closing: end (host) or leave (guest) its meeting.
pub fn tab_closed(label: &str) {
    stop_session(label);
}

/// From the meeting page: `data` to one member (`to`) or everyone (`to` = null).
#[tauri::command]
pub async fn meet_send(webview: Webview, to: Option<String>, data: Value) -> Result<(), AppError> {
    let s = SESSIONS.lock().unwrap();
    match s.get(webview.label()) {
        Some(Session::Host(h)) => h.route("host", to.as_deref(), data),
        Some(Session::Guest(g)) => {
            let m = match to {
                Some(to) => json!({ "t": "to", "to": to, "data": data }),
                None => json!({ "t": "all", "data": data }),
            };
            let _ = g.tx.send(m);
        }
        None => return Err(AppError::new("참여 중인 회의가 없습니다.", "no session")),
    }
    Ok(())
}

/// Host: lets a knocking guest in (or not).
#[tauri::command]
pub async fn meet_admit(webview: Webview, id: String, accept: bool) -> Result<(), AppError> {
    match SESSIONS.lock().unwrap().get(webview.label()) {
        Some(Session::Host(h)) => {
            h.admit(&id, accept);
            Ok(())
        }
        _ => Err(AppError::new("회의를 연 사람만 수락할 수 있습니다.", "not host")),
    }
}

/// Leaves (guest) or ends (host) the meeting.
#[tauri::command]
pub async fn meet_leave(webview: Webview) -> Result<(), AppError> {
    stop_session(webview.label());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_round_trip_and_sealing() {
        let inv = Invite { r: "a1b2c3".into(), k: B64.encode([7u8; 16]), a: vec!["203.0.113.5:47800".into(), "192.168.0.10:47800".into()] };
        let code = invite_code(&inv);
        assert!(code.starts_with(PREFIX));
        let back = parse_invite(&format!("  {}\n", code)).unwrap();
        assert_eq!(back.a, inv.a);
        let (k1, k2) = (Sealer::new(&inv).unwrap(), Sealer::new(&back).unwrap());
        let sealed = k1.seal(&json!({ "t": "knock", "name": "민수" }));
        assert_eq!(k2.open(&sealed).unwrap()["name"], "민수");
        // Another room's key cannot read or forge frames.
        let other = Sealer::new(&Invite { r: "zzz".into(), k: B64.encode([7u8; 16]), a: vec!["x".into()] }).unwrap();
        assert!(other.open(&sealed).is_none());
        assert!(parse_invite("KITE-MEET1.abc").is_err());
    }

    /// Collects the events a page would get.
    fn sink() -> (Sink, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Arc::new(move |v: Value| drop(tx.send(v["msg"].clone()))), rx)
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<Value>, t: &str) -> Value {
        loop {
            let v = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event in time").expect("open");
            if v["t"] == t {
                return v;
            }
        }
    }

    /// Real TCP on this machine: knock, accept, relay both ways, a second member, the end.
    #[tokio::test(flavor = "multi_thread")]
    async fn meeting_over_tcp() {
        if local_ip().is_none() {
            return; // no network here
        }
        let (hs, mut hrx) = sink();
        let (host, info) = host_start(hs, "호스트", false).await.unwrap();
        assert!(info.code.starts_with(PREFIX) && info.public.is_none() && info.blocked == Some("no-upnp"));

        let (gs, mut grx) = sink();
        let (guest, _) = join_start(gs, &info.code, "민수").await.unwrap();
        let knock = next(&mut hrx, "knock").await;
        assert_eq!(knock["name"], "민수");
        let id = knock["id"].as_str().unwrap().to_string();
        // Not admitted yet: what it sends is ignored.
        let _ = guest.tx.send(json!({ "t": "to", "to": "host", "data": { "k": "early" } }));
        tokio::time::sleep(Duration::from_millis(300)).await;
        host.admit(&id, true);
        let admit = next(&mut grx, "admit").await;
        assert_eq!(admit["you"], id.as_str());
        assert_eq!(admit["peers"][0]["id"], "host");
        assert_eq!(next(&mut hrx, "joined").await["id"], id.as_str());

        // Guest → host page, host page → guest.
        let _ = guest.tx.send(json!({ "t": "to", "to": "host", "data": { "k": "sdp", "sdp": "x" } }));
        let got = next(&mut hrx, "from").await;
        assert_eq!((got["from"].as_str(), got["data"]["k"].as_str()), (Some(id.as_str()), Some("sdp")));
        host.route("host", Some(&id), json!({ "k": "chat", "text": "안녕" }));
        let got = next(&mut grx, "from").await;
        assert_eq!((got["from"].as_str(), got["data"]["text"].as_str()), (Some("host"), Some("안녕")));

        // A second member: refused once, then let in; the first one hears about it.
        let (g2s, mut g2rx) = sink();
        let (_g2, _) = join_start(g2s, &info.code, "지영").await.unwrap();
        let k2 = next(&mut hrx, "knock").await;
        host.admit(k2["id"].as_str().unwrap(), false);
        next(&mut g2rx, "deny").await;
        let (g3s, mut g3rx) = sink();
        let (g3, _) = join_start(g3s, &info.code, "지영").await.unwrap();
        let k3 = next(&mut hrx, "knock").await;
        let id3 = k3["id"].as_str().unwrap().to_string();
        host.admit(&id3, true);
        let a3 = next(&mut g3rx, "admit").await;
        assert_eq!(a3["peers"].as_array().unwrap().len(), 2, "host and 민수");
        assert_eq!(next(&mut grx, "joined").await["id"], id3.as_str());
        // Broadcast from the new member reaches both others.
        let _ = g3.tx.send(json!({ "t": "all", "data": { "k": "state" } }));
        assert_eq!(next(&mut grx, "from").await["from"], id3.as_str());
        assert_eq!(next(&mut hrx, "from").await["from"], id3.as_str());

        // Leaving and ending.
        g3.stop.cancel();
        assert_eq!(next(&mut grx, "left").await["id"], id3.as_str());
        host.stop.cancel();
        next(&mut grx, "closed").await;
    }

    /// Manual: does this network's router answer UPnP? (reads only, opens nothing)
    #[tokio::test]
    #[ignore]
    async fn probe_router() {
        let mut opts = igd_next::SearchOptions::default();
        opts.timeout = Some(Duration::from_secs(4));
        match igd_next::aio::tokio::search_gateway(opts).await {
            Ok(gw) => println!("UPnP router {} external internal={:?}", gw.addr, gw.get_external_ip().await.map(is_internal)),
            Err(e) => println!("no UPnP router: {e}"),
        }
    }

    #[tokio::test]
    async fn knock_admit_and_relay() {
        let inv = Invite { r: "r1".into(), k: B64.encode([1u8; 16]), a: vec![] };
        let key = Sealer::new(&inv).unwrap();
        let (a, b) = tokio::io::duplex(4096);
        let (mut ar, mut aw) = tokio::io::split(a);
        let (mut br, mut bw) = tokio::io::split(b);
        write_frame(&mut aw, &key, &json!({ "t": "to", "to": "host", "data": { "k": "sdp" } })).await.unwrap();
        let got = read_frame(&mut br, &key).await.unwrap();
        assert_eq!(got["data"]["k"], "sdp");
        // A frame from someone without the code is dropped.
        let bad = Sealer::new(&Invite { r: "r2".into(), k: B64.encode([1u8; 16]), a: vec![] }).unwrap();
        write_frame(&mut bw, &bad, &json!({ "t": "knock" })).await.unwrap();
        assert!(read_frame(&mut ar, &key).await.is_none());
    }
}
