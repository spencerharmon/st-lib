//! Common Non Session Manager (NSM) client for st-suite components.
//!
//! Spins up a tokio task that:
//!   - reads `NSM_URL` from the environment,
//!   - sends `/nsm/server/announce`,
//!   - dispatches `/nsm/client/{save,open,show_optional_gui,hide_optional_gui,
//!     session_is_loaded}` into a `tokio::sync::mpsc::Receiver<Event>` for
//!     the caller,
//!   - sends `/reply` *after* the caller acknowledges completion (so the
//!     reply ordering follows the NSM spec),
//!   - lets the caller broadcast `/nsm/client/is_dirty` / `/nsm/client/is_clean`
//!     and `/nsm/client/gui_is_shown` / `/nsm/client/gui_is_hidden` on demand.
//!
//! Async runtime: tokio. If `NSM_URL` is unset the client launches a stub
//! that emits nothing — callers can treat NSM as optional.
//!
//! Wire-level details that bit st-loop's hand-rolled version:
//! - Uses [`tokio::net::UdpSocket`] (no blocking recv inside an async task).
//! - Replies to Save/Open are sent only after the caller drops the
//!   [`Ack`] (or calls [`Ack::ok`] / [`Ack::err`]) — so the server learns
//!   the operation is done, not just queued.
//!
//! See `plan.org` "NSM" section for the audit that motivated this.

use std::env;
use std::net::SocketAddr;
use std::process;

use rosc::encoder;
use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

/// Capability flags advertised in the announce message. Combine with `|`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Capabilities {
	/// `:switch:` — the client can replace its loaded session in-place
	/// without restarting (handle a second `/nsm/client/open`).
	pub switch: bool,
	/// `:dirty:` — the client emits `/nsm/client/is_dirty` /
	/// `/nsm/client/is_clean` when its modified state changes.
	pub dirty: bool,
	/// `:optional-gui:` — the client has an optional GUI that NSM can
	/// show/hide via `/nsm/client/{show,hide}_optional_gui`.
	pub optional_gui: bool,
	/// `:progress:` — the client emits `/nsm/client/progress`. Not used
	/// by any st-suite component yet, but cheap to expose.
	pub progress: bool,
	/// `:message:` — the client emits `/nsm/client/message`.
	pub message: bool,
}

impl Capabilities {
	pub fn to_caps_string(&self) -> String {
		let mut s = String::new();
		if self.switch       { s.push_str(":switch:"); }
		if self.dirty        { s.push_str(":dirty:"); }
		if self.optional_gui { s.push_str(":optional-gui:"); }
		if self.progress     { s.push_str(":progress:"); }
		if self.message      { s.push_str(":message:"); }
		s
	}
}

/// Events delivered to the caller on the [`Client::rx`] channel.
#[derive(Debug)]
pub enum Event {
	/// NSM is asking the client to open / switch to a session.
	///
	/// `path` is a directory or file path *prefix* the client should
	/// own (NSM picks it; the client appends its own filenames).
	///
	/// Drop `ack` or call [`Ack::ok`] / [`Ack::err`] when the open is
	/// truly complete (audio loaded, etc.) — the `/reply` is only sent
	/// at that point.
	Open {
		path: String,
		display_name: String,
		client_id: String,
		ack: Ack,
	},
	/// NSM is asking the client to save its session.
	Save { ack: Ack },
	/// NSM wants the optional GUI shown. Caller must subsequently call
	/// [`Handle::gui_shown`] once the window is up.
	ShowGui,
	/// NSM wants the optional GUI hidden. Caller must subsequently call
	/// [`Handle::gui_hidden`].
	HideGui,
	/// All session clients have finished loading (informational).
	SessionLoaded,
	/// NSM rejected the announce (e.g. wrong API version).
	AnnounceError { code: i32, message: String },
	/// Announce accepted, session manager identified itself.
	AnnounceOk { manager_name: String, session_name: String, session_path: String },
}

/// Completion handle for a Save or Open event. Reply is sent when this
/// is dropped (success by default) or explicitly via [`Ack::ok`] /
/// [`Ack::err`].
pub struct Ack {
	tx: Option<oneshot::Sender<Result<String, (i32, String)>>>,
	addr: String,
}

impl Ack {
	/// Signal success with a human-readable message.
	pub fn ok<S: Into<String>>(mut self, message: S) {
		if let Some(tx) = self.tx.take() {
			let _ = tx.send(Ok(message.into()));
		}
	}
	/// Signal failure. `code` follows NSM error code conventions
	/// (negative integers; see NSM API docs).
	pub fn err<S: Into<String>>(mut self, code: i32, message: S) {
		if let Some(tx) = self.tx.take() {
			let _ = tx.send(Err((code, message.into())));
		}
	}
}

impl Drop for Ack {
	fn drop(&mut self) {
		if let Some(tx) = self.tx.take() {
			let _ = tx.send(Ok(String::new()));
		}
	}
}

impl std::fmt::Debug for Ack {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Ack").field("addr", &self.addr).finish()
	}
}

/// Outbound command from the caller (GUI thread, dispatcher, ...) to
/// the NSM task.
#[derive(Debug)]
enum Outbound {
	Dirty,
	Clean,
	GuiShown,
	GuiHidden,
	Progress(f32),
	Message { priority: i32, text: String },
}

/// Handle held by the caller to push status updates back to NSM.
#[derive(Clone)]
pub struct Handle {
	tx: mpsc::Sender<Outbound>,
}

impl Handle {
	pub async fn dirty(&self)      { let _ = self.tx.send(Outbound::Dirty).await; }
	pub async fn clean(&self)      { let _ = self.tx.send(Outbound::Clean).await; }
	pub async fn gui_shown(&self)  { let _ = self.tx.send(Outbound::GuiShown).await; }
	pub async fn gui_hidden(&self) { let _ = self.tx.send(Outbound::GuiHidden).await; }
	pub async fn progress(&self, frac: f32) {
		let _ = self.tx.send(Outbound::Progress(frac)).await;
	}
	pub async fn message<S: Into<String>>(&self, priority: i32, text: S) {
		let _ = self.tx.send(Outbound::Message { priority, text: text.into() }).await;
	}
}

/// Builder for [`Client`].
pub struct Builder {
	client_name: String,
	executable: String,
	capabilities: Capabilities,
}

impl Builder {
	pub fn new<S: Into<String>>(client_name: S) -> Self {
		let name = client_name.into();
		Self {
			executable: name.clone(),
			client_name: name,
			capabilities: Capabilities::default(),
		}
	}
	pub fn executable<S: Into<String>>(mut self, exe: S) -> Self {
		self.executable = exe.into();
		self
	}
	pub fn capabilities(mut self, caps: Capabilities) -> Self {
		self.capabilities = caps;
		self
	}
	/// Spawn the NSM task. Returns a [`Client`] (receiver of events) and
	/// a [`Handle`] (for pushing status updates back). If `NSM_URL` is
	/// unset the task immediately exits and no events will arrive.
	pub fn launch(self) -> (Client, Handle) {
		let (evt_tx, evt_rx) = mpsc::channel::<Event>(16);
		let (out_tx, out_rx) = mpsc::channel::<Outbound>(16);

		tokio::spawn(async move {
			if let Err(e) = run(self, evt_tx, out_rx).await {
				eprintln!("[nsm] task exited: {e}");
			}
		});

		(Client { rx: evt_rx }, Handle { tx: out_tx })
	}
}

/// Receives [`Event`]s from the NSM task. Caller is expected to
/// `.recv()` in its event loop and respond appropriately.
pub struct Client {
	pub rx: mpsc::Receiver<Event>,
}

// ---------------------------------------------------------------------
// Internal driver
// ---------------------------------------------------------------------

async fn run(
	cfg: Builder,
	evt_tx: mpsc::Sender<Event>,
	mut out_rx: mpsc::Receiver<Outbound>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let nsm_url = match env::var("NSM_URL") {
		Ok(u) => u,
		Err(_) => {
			// No NSM session active. Silently exit; the caller's rx
			// will just never receive anything.
			return Ok(());
		}
	};

	let server_addr = parse_nsm_url(&nsm_url)?;
	let sock = UdpSocket::bind("127.0.0.1:0").await?;
	println!("[nsm] connecting to {server_addr} (NSM_URL={nsm_url})");

	// Track outstanding reply oneshots so we can send /reply when the
	// caller signals completion.
	let (ack_tx, mut ack_rx) =
		mpsc::channel::<(String, oneshot::Receiver<Result<String, (i32, String)>>)>(16);

	// Send announce.
	let announce = encode_msg("/nsm/server/announce", vec![
		OscType::from(cfg.client_name.clone()),
		OscType::from(cfg.capabilities.to_caps_string()),
		OscType::from(cfg.executable.clone()),
		OscType::from(1_i32), // api_version_major
		OscType::from(2_i32), // api_version_minor (NSM is 1.2)
		OscType::from(process::id() as i32),
	]);
	sock.send_to(&announce, server_addr).await?;

	let mut buf = vec![0u8; rosc::decoder::MTU];

	// A small task-local set of pending acks awaiting their /reply.
	// Use a tokio FuturesUnordered-style pattern by spawning a watcher
	// per ack that forwards completion onto a shared channel.
	let (reply_tx, mut reply_rx) =
		mpsc::channel::<(String, Result<String, (i32, String)>)>(16);

	loop {
		tokio::select! {
			r = sock.recv_from(&mut buf) => {
				let (size, _from) = match r {
					Ok(v) => v,
					Err(e) => { eprintln!("[nsm] recv error: {e}"); break; }
				};
				if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..size]) {
					handle_incoming(packet, &cfg, &evt_tx, &ack_tx).await;
				}
			}

			Some(cmd) = out_rx.recv() => {
				let bytes = match cmd {
					Outbound::Dirty      => encode_msg("/nsm/client/is_dirty", vec![]),
					Outbound::Clean      => encode_msg("/nsm/client/is_clean", vec![]),
					Outbound::GuiShown   => encode_msg("/nsm/client/gui_is_shown", vec![]),
					Outbound::GuiHidden  => encode_msg("/nsm/client/gui_is_hidden", vec![]),
					Outbound::Progress(p)=> encode_msg("/nsm/client/progress", vec![OscType::from(p)]),
					Outbound::Message{priority,text} => encode_msg(
						"/nsm/client/message",
						vec![OscType::from(priority), OscType::from(text)],
					),
				};
				let _ = sock.send_to(&bytes, server_addr).await;
			}

			// A new ack registered. Spawn a watcher that forwards its
			// completion to reply_rx with the original message addr.
			Some((addr, ack)) = ack_rx.recv() => {
				let reply_tx = reply_tx.clone();
				tokio::spawn(async move {
					let res = ack.await.unwrap_or(Ok(String::new()));
					let _ = reply_tx.send((addr, res)).await;
				});
			}

			Some((addr, result)) = reply_rx.recv() => {
				let bytes = match result {
					Ok(msg) => encode_msg("/reply", vec![
						OscType::from(addr),
						OscType::from(msg),
					]),
					Err((code, msg)) => encode_msg("/error", vec![
						OscType::from(addr),
						OscType::from(code),
						OscType::from(msg),
					]),
				};
				let _ = sock.send_to(&bytes, server_addr).await;
			}
		}
	}

	Ok(())
}

async fn handle_incoming(
	packet: OscPacket,
	_cfg: &Builder,
	evt_tx: &mpsc::Sender<Event>,
	ack_tx: &mpsc::Sender<(String, oneshot::Receiver<Result<String, (i32, String)>>)>,
) {
	match packet {
		OscPacket::Message(msg) => match msg.addr.as_str() {
			"/nsm/client/save" => {
				let (tx, rx) = oneshot::channel();
				let ack = Ack { tx: Some(tx), addr: msg.addr.clone() };
				let _ = ack_tx.send((msg.addr.clone(), rx)).await;
				let _ = evt_tx.send(Event::Save { ack }).await;
			}
			"/nsm/client/open" => {
				let path         = arg_string(&msg, 0).unwrap_or_default();
				let display_name = arg_string(&msg, 1).unwrap_or_default();
				let client_id    = arg_string(&msg, 2).unwrap_or_default();
				let (tx, rx) = oneshot::channel();
				let ack = Ack { tx: Some(tx), addr: msg.addr.clone() };
				let _ = ack_tx.send((msg.addr.clone(), rx)).await;
				let _ = evt_tx.send(Event::Open {
					path, display_name, client_id, ack,
				}).await;
			}
			"/nsm/client/show_optional_gui" => {
				let _ = evt_tx.send(Event::ShowGui).await;
			}
			"/nsm/client/hide_optional_gui" => {
				let _ = evt_tx.send(Event::HideGui).await;
			}
			"/nsm/client/session_is_loaded" => {
				let _ = evt_tx.send(Event::SessionLoaded).await;
			}
			"/reply" => {
				// Reply to our /nsm/server/announce arrives as
				// /reply "/nsm/server/announce" "<msg>" "<mgr_name>" "<caps>".
				if matches!(arg_string(&msg, 0).as_deref(), Some("/nsm/server/announce")) {
					let manager_name = arg_string(&msg, 2).unwrap_or_default();
					let session_name = arg_string(&msg, 1).unwrap_or_default();
					let _ = evt_tx.send(Event::AnnounceOk {
						manager_name,
						session_name,
						session_path: String::new(),
					}).await;
				}
			}
			"/error" => {
				if matches!(arg_string(&msg, 0).as_deref(), Some("/nsm/server/announce")) {
					let code = arg_int(&msg, 1).unwrap_or(0);
					let message = arg_string(&msg, 2).unwrap_or_default();
					let _ = evt_tx.send(Event::AnnounceError { code, message }).await;
				}
			}
			_ => {
				// Unknown — ignore.
			}
		},
		OscPacket::Bundle(b) => {
			for p in b.content {
				Box::pin(handle_incoming(p, _cfg, evt_tx, ack_tx)).await;
			}
		}
	}
}

fn parse_nsm_url(url: &str) -> Result<SocketAddr, Box<dyn std::error::Error + Send + Sync>> {
	// NSM_URL looks like "osc.udp://host:port/" — strip scheme + trailing slash.
	let s = url.trim();
	let s = s.trim_end_matches('/');
	let host_port = s
		.rsplit("//")
		.next()
		.ok_or("NSM_URL: missing scheme separator")?;
	let host_port = host_port.split('/').next().unwrap_or(host_port);
	Ok(host_port.parse()?)
}

fn encode_msg(addr: &str, args: Vec<OscType>) -> Vec<u8> {
	encoder::encode(&OscPacket::Message(OscMessage {
		addr: addr.to_string(),
		args,
	})).expect("encode osc")
}

fn arg_string(msg: &OscMessage, idx: usize) -> Option<String> {
	msg.args.get(idx).and_then(|a| a.clone().string())
}

fn arg_int(msg: &OscMessage, idx: usize) -> Option<i32> {
	msg.args.get(idx).and_then(|a| match a {
		OscType::Int(i) => Some(*i),
		_ => None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn caps_string() {
		let c = Capabilities { switch: true, optional_gui: true, ..Default::default() };
		assert_eq!(c.to_caps_string(), ":switch::optional-gui:");
		assert_eq!(Capabilities::default().to_caps_string(), "");
	}

	#[test]
	fn parses_nsm_url_trailing_slash() {
		let a = parse_nsm_url("osc.udp://127.0.0.1:14999/").unwrap();
		assert_eq!(a.to_string(), "127.0.0.1:14999");
	}

	#[test]
	fn parses_nsm_url_no_trailing_slash() {
		let a = parse_nsm_url("osc.udp://192.168.1.5:6000").unwrap();
		assert_eq!(a.to_string(), "192.168.1.5:6000");
	}
}
