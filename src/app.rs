use super::display;
use anyhow::Error;
use futures_util::{SinkExt, StreamExt};
use std::borrow::Cow;
use std::cell::RefCell;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, UnixListener};
use tokio::select;
use tokio::signal::unix::Signal;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite;
use tokio_util::net::Listener;
use tokio_util::sync::CancellationToken;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::frame::CloseFrame;
use tungstenite::Message;

/// Settings that are used by connections.
#[derive(Debug, Eq, Hash, PartialEq)]
struct Settings {
	/// The permitted origins.
	pub origin: Vec<String>,

	/// The time between pings.
	pub ping_time: Duration,
}

/// A future which polls a [`JoinSet`](JoinSet) which is held behind an [`Rc`](Rc) of a
/// [`RefCell`](RefCell), without holding a borrow of the `Rc` across a yield point.
///
/// The lifetime `'a` is the lifetime of the `Rc` that is borrowed (to avoid excessive reference
/// count manipulation) by this value.
struct JoinSetPoller<'a, T: 'static>(&'a Rc<RefCell<JoinSet<T>>>);

impl<'a, T: 'static> JoinSetPoller<'a, T> {
	/// Creates a new `JoinSetPoller`.
	pub fn new(join_set: &'a Rc<RefCell<JoinSet<T>>>) -> Self {
		Self(join_set)
	}
}

impl<T: 'static> Future for JoinSetPoller<'_, T> {
	type Output = Option<Result<T, tokio::task::JoinError>>;

	fn poll(
		self: Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Self::Output> {
		self.0.borrow_mut().poll_join_next(cx)
	}
}

/// A [`JoinSet`](JoinSet) for tasks which is shared among the tasks.
type SharedJoinSet = Rc<RefCell<JoinSet<Result<(), Error>>>>;

/// Checks whether a given error kind is fatal or not.
///
/// A fatal error is one caused by resource exhaustion or program or configuration error. A
/// non-fatal error is one that can be caused by the remote peer and should result in the specific
/// peer connection being closed but the application as a whole continuing to operate.
fn is_fatal(e: std::io::ErrorKind) -> bool {
	use std::io::ErrorKind;
	matches!(
		e,
		ErrorKind::NotConnected
			| ErrorKind::AddrInUse
			| ErrorKind::AddrNotAvailable
			| ErrorKind::InvalidInput
			| ErrorKind::InvalidData
			| ErrorKind::Unsupported
			| ErrorKind::OutOfMemory
	)
}

/// An individual mouse event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum MouseEvent {
	/// A click.
	Click,

	/// A motion.
	Move(i16, i16),
}

impl MouseEvent {
	/// The length, in bytes, of a mouse event’s encoded form.
	const ENCODED_SIZE: usize = 6;
}

impl TryFrom<[u8; MouseEvent::ENCODED_SIZE]> for MouseEvent {
	type Error = Error;

	fn try_from(value: [u8; MouseEvent::ENCODED_SIZE]) -> Result<Self, Self::Error> {
		let opcode = u16::from_be_bytes([value[0], value[1]]);
		let param1 = i16::from_be_bytes([value[2], value[3]]);
		let param2 = i16::from_be_bytes([value[4], value[5]]);
		match opcode {
			0 => Ok(Self::Click),
			1 => Ok(Self::Move(param1, param2)),
			_ => Err(Error::msg("Invalid opcode in packet")),
		}
	}
}

/// A future that waits until either a signal is received or shutdown is scheduled by another
/// means, then returns `Ok(())`.
async fn monitor_signal(mut signal: Signal, cancel: CancellationToken) -> Result<(), Error> {
	select! {
		_ = signal.recv() => cancel.cancel(),
		_ = cancel.cancelled() => (),
	};
	Ok(())
}

/// A future that accepts incoming connections and dispatches them for handling until shutdown is
/// scheduled, then returns `Ok(())`.
async fn monitor_listener<L: Listener>(
	mut listener: L,
	cancel: CancellationToken,
	join_set: SharedJoinSet,
	event_sender: mpsc::UnboundedSender<MouseEvent>,
	settings: Rc<Settings>,
) -> Result<(), Error>
where
	L::Addr: std::fmt::Debug,
	L::Io: 'static + Unpin,
{
	loop {
		select! {
			_ = cancel.cancelled() => break,
			result = listener.accept() => match result {
				Ok((stream, addr)) => {
					log::info!("Accepted connection from {addr:?}");
					join_set.borrow_mut().spawn_local(monitor_stream(stream, cancel.clone(), event_sender.clone(), settings.clone()));
				}
				Err(e) => if is_fatal(e.kind()) {
					return Err(e.into());
				} else {
					log::error!("Error accepting connection: {e}");
				}
			}
		}
	}
	Ok(())
}

/// The configuration for WebSocket connections.
#[allow(deprecated)] // max_send_queue is deprecated but there is no alternative; we must provide
					 // some value, and Default::default() is non-const!
const WEBSOCKET_CONFIG: tungstenite::protocol::WebSocketConfig =
	tungstenite::protocol::WebSocketConfig {
		max_send_queue: None,
		write_buffer_size: 0,
		max_write_buffer_size: 64,
		max_message_size: Some(MouseEvent::ENCODED_SIZE),
		max_frame_size: Some(MouseEvent::ENCODED_SIZE),
		accept_unmasked_frames: false,
	};

/// Constructs a Tungstenite rejection response with a plain-text body.
fn make_rejection(
	status: tungstenite::http::status::StatusCode,
	message: &str,
) -> tungstenite::handshake::server::ErrorResponse {
	use tungstenite::http::header;
	tungstenite::http::response::Builder::new()
		.status(status)
		.header(
			header::CONTENT_TYPE,
			header::HeaderValue::from_static("text/plain; charset=UTF-8"),
		)
		.body(Some(message.to_owned()))
		.unwrap()
}

/// The subprotocol we understand.
const SUBPROTOCOL: &str = "ca.chead.remote-mouse.packets";

/// A Tungstenite request handler callback.
struct TungsteniteCallback<'a> {
	/// The settings.
	settings: &'a Rc<Settings>,
}

impl tungstenite::handshake::server::Callback for TungsteniteCallback<'_> {
	fn on_request(
		self,
		request: &tungstenite::handshake::server::Request,
		mut response: tungstenite::handshake::server::Response,
	) -> Result<
		tungstenite::handshake::server::Response,
		tungstenite::handshake::server::ErrorResponse,
	> {
		use tungstenite::http::header;
		use tungstenite::http::status::StatusCode;

		let protocol = request.headers().get(header::SEC_WEBSOCKET_PROTOCOL);
		let Some(protocol) = protocol else {
			return Err(make_rejection(StatusCode::BAD_REQUEST, "Subprotocol not specified"));
		};
		if protocol != SUBPROTOCOL {
			return Err(make_rejection(
				StatusCode::BAD_REQUEST,
				"Requested subprotocol not supported",
			));
		}
		let allowed_origins = &self.settings.origin;
		if !allowed_origins.is_empty() {
			let origin = request.headers().get(header::ORIGIN);
			let Some(origin) = origin else {
				return Err(make_rejection(StatusCode::BAD_REQUEST, "Origin not specified"));
			};
			if !allowed_origins.iter().any(|i| i == origin) {
				return Err(make_rejection(
					StatusCode::FORBIDDEN,
					"Non-permitted origin",
				));
			}
		}
		response.headers_mut().insert(
			header::SEC_WEBSOCKET_PROTOCOL,
			header::HeaderValue::from_static(SUBPROTOCOL),
		);
		Ok(response)
	}
}

/// A future that communicates over a WebSocket connection and sends decoded events to the event
/// handler task, until either the peer closes the socket or a shutdown is scheduled, then returns
/// `Ok(())`.
async fn monitor_stream<S: AsyncRead + AsyncWrite + Unpin>(
	stream: S,
	cancel: CancellationToken,
	event_sender: mpsc::UnboundedSender<MouseEvent>,
	settings: Rc<Settings>,
) -> Result<(), Error> {
	use tungstenite::error::Error;

	let inner = async move {
		// Do the handshake.
		let mut socket = tokio_tungstenite::accept_hdr_async_with_config(
			stream,
			TungsteniteCallback {
				settings: &settings,
			},
			Some(WEBSOCKET_CONFIG),
		)
		.await?;
		log::trace!("Handshake complete");

		// Run the socket until closed or until a shutdown signal arrives.
		let close_frame = loop {
			select! {
				_ = cancel.cancelled() => {
					// A shutdown has been signalled.
					break Some(CloseFrame {
						code: CloseCode::Away,
						reason: Cow::Borrowed("Server shutting down"),
					});
				}
				_ = tokio::time::sleep(settings.ping_time), if settings.ping_time != Duration::ZERO => {
					// A while has passed since anything was received. Send a ping, which will
					// provoke a pong, to ensure that no intermediaries time out the connection.
					socket.send(Message::Ping(Vec::new())).await?;
				}
				message = socket.next() => match message {
					None => {
						// The TCP connection was closed without a WebSocket close frame.
						log::info!("Connection closed without close frame");
						break None;
					}
					Some(Ok(m)) => match handle_message(&m, &event_sender) {
						ControlFlow::Break(cf) => break cf,
						ControlFlow::Continue(()) => (),
					}
					Some(Err(e)) => {
						// An error occurred.
						break match e {
							Error::ConnectionClosed | Error::AlreadyClosed | Error::Tls(_) | Error::Url(_) | Error::Http(_) => {
								// These should never happen. ConnectionClosed and
								// AlreadyClosed are reported via a None value instead. Tls
								// should be impossible because we don’t use TLS. Url and Http
								// should be impossible because they only happen on clients,
								// not servers.
								panic!("Impossible enum value");
							}
							Error::Io(_) | Error::WriteBufferFull(_) | Error::AttackAttempt => return Err(e),
							Error::Capacity(e) => Some(CloseFrame {
								code: CloseCode::Size,
								reason: Cow::Owned(format!("{e}")),
							}),
							Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake) => None,
							Error::Protocol(e) => Some(CloseFrame {
								code: CloseCode::Protocol,
								reason: Cow::Owned(format!("{e}")),
							}),
							Error::Utf8 => Some(CloseFrame {
								code: CloseCode::Invalid,
								reason: Cow::Borrowed("Invalid UTF-8"),
							}),
							Error::HttpFormat(e) => Some(CloseFrame {
								code: CloseCode::Protocol,
								reason: Cow::Owned(format!("{e}")),
							}),
						}
					}
				}
			}
		};

		// Send the close frame.
		if let Some(close_frame) = close_frame {
			log::info!(
				"Closing connection (code {}, reason {})",
				close_frame.code,
				close_frame.reason
			);
			socket.send(Message::Close(Some(close_frame))).await?;
		} else {
			log::info!("Closing connection without a further close frame");
		}

		// Read any remaining messages.
		while let Some(Ok(message)) = socket.next().await {
			// There’s not really anything we can do about anything unexpected at this point, since
			// we’ve already sent a close frame, so skip the error handling and just drop anything
			// wrong.
			if let Message::Binary(data) = message {
				// Check if the message is the right length.
				if let Ok(data) = <[u8; MouseEvent::ENCODED_SIZE]>::try_from(data) {
					// The message is the right length. Try to decode it.
					if let Ok(data) = MouseEvent::try_from(data) {
						// The message is OK. Send it to the event handling task.
						let _ = event_sender.send(data);
					}
				}
			}
		}

		Ok(())
	};

	match inner.await {
		Ok(()) => Ok(()),
		Err(e) => match e {
			Error::AlreadyClosed => Err(e.into()),
			Error::Io(ref io_error) if is_fatal(io_error.kind()) => {
				Err(e.into())
			}
			_ => {
				log::error!("Error in connection: {e}");
				Ok(())
			}
		},
	}
}

/// Handles a single message received on a stream.
///
/// Returns [`Continue`](ControlFlow::Continue) if the loop should continue and receive another
/// message, [`Break`](ControlFlow::Break) with the [`CloseFrame`](CloseFrame) to reply with (or
/// `None` to not send a [`CloseFrame`](CloseFrame)) if the socket should shut down, or `Err` in
/// the event of a fatal application-wide error.
fn handle_message(
	message: &Message,
	event_sender: &mpsc::UnboundedSender<MouseEvent>,
) -> ControlFlow<Option<CloseFrame<'static>>, ()> {
	// A message was received, an error occurred, or the connection was closed.
	log::trace!("Socket read returned {message:?}");
	match message {
		Message::Text(_) => {
			// Text frames are not supported.
			ControlFlow::Break(Some(CloseFrame {
				code: CloseCode::Unsupported,
				reason: Cow::Borrowed("Text frames not supported"),
			}))
		}
		Message::Binary(data) => {
			// Check if the message is the right length.
			let data_len = data.len();
			if let Ok(data) = <[u8; MouseEvent::ENCODED_SIZE]>::try_from(data.as_slice()) {
				// The message is the right length. Try to decode it.
				match MouseEvent::try_from(data) {
					Ok(data) => {
						// The message is OK. Send it to the event handling task.
						let _ = event_sender.send(data);
						ControlFlow::Continue(())
					}
					Err(e) => {
						// The message is erroneous.
						ControlFlow::Break(Some(CloseFrame {
							code: CloseCode::Protocol,
							reason: Cow::Owned(format!("Undecodable event packet: {e}")),
						}))
					}
				}
			} else {
				// The message is the wrong length.
				ControlFlow::Break(Some(CloseFrame {
					code: CloseCode::Protocol,
					reason: Cow::Owned(format!("Wrong-sized ({data_len}) binary frame")),
				}))
			}
		}
		Message::Ping(_) | Message::Pong(_) => ControlFlow::Continue(()), // Ignore these.
		Message::Close(close_frame) => {
			// The client asked to close the connection. Do that.
			match close_frame {
				None => {
					log::info!("Client requested connection close without details");
					ControlFlow::Break(None)
				}
				Some(close_frame) => {
					log::info!(
						"Client requested connection close with code {}, reason {}",
						close_frame.code,
						close_frame.reason
					);
					ControlFlow::Break(None)
				}
			}
		}
		Message::Frame(_) => {
			// This should never happen for receiving.
			panic!("Impossible enum value");
		}
	}
}

/// A handler for mouse events.
struct EventHandler {
	/// The display connection.
	display_connection: display::Connection,

	/// The mouse motion accumulated so far.
	motion: (i16, i16),
}

impl EventHandler {
	/// Constructs a new `EventHandler`.
	pub fn new(display_connection: display::Connection) -> Self {
		Self {
			display_connection,
			motion: (0, 0),
		}
	}

	/// Handles an event.
	pub fn handle_event(&mut self, event: MouseEvent) -> Result<(), Error> {
		match event {
			MouseEvent::Click => {
				// Flush any cached motion first before clicking.
				self.flush()?;
				self.display_connection.click()?;
			}
			MouseEvent::Move(x, y) => {
				// To avoid spamming the X server with too many warp requests, accumulate incoming
				// motion and don’t send it to the X server until a flush or click event, or if the
				// accumulated motion overflows the i16 data type.
				if self.motion.0.checked_add(x).is_none() || self.motion.1.checked_add(y).is_none()
				{
					self.flush()?;
				}
				self.motion = (self.motion.0 + x, self.motion.1 + y);
			}
		}
		Ok(())
	}

	/// Flushes any accumulated motion.
	pub fn flush(&mut self) -> Result<(), Error> {
		// Only actually send the motion if there is any to send.
		if self.motion != (0, 0) {
			self.display_connection.warp(self.motion.0, self.motion.1)?;
			self.motion = (0, 0);
		}
		Ok(())
	}
}

/// A future that receives mouse events from an MPSC channel and executes them on a display
/// connection.
///
/// This future returns `Ok(())` when the MPSC channel is closed.
async fn handle_events(
	mut receiver: mpsc::UnboundedReceiver<MouseEvent>,
	display_connection: display::Connection,
) -> Result<(), Error> {
	let mut handler = EventHandler::new(display_connection);
	while let Some(event) = receiver.recv().await {
		// Handle this event.
		handler.handle_event(event)?;

		// Handle any more events that are immediately ready.
		while let Ok(event) = receiver.try_recv() {
			handler.handle_event(event)?;
		}

		// There are no more events immediately ready, so we’re about to go to sleep. To avoid
		// delaying motion indefinitely, flush it before sleeping.
		handler.flush()?;
	}
	Ok(())
}

/// Runs the application once all basic resources have been obtained.
pub async fn run(
	display_connection: display::Connection,
	signals: [Signal; 2],
	tcp_listeners: Vec<TcpListener>,
	unix_listeners: Vec<UnixListener>,
	origin: Vec<String>,
	ping_time: Duration,
) -> Result<(), Error> {
	// Pack up the settings behind an Rc so they can be reused by all the connection handlers.
	let settings = Rc::new(Settings { origin, ping_time });

	// Create a cancellation token for use by all subordinate tasks.
	let cancel = CancellationToken::new();

	// Create an MPSC channel which will carry mouse events from the connection to the central
	// event handler.
	let (event_sender, event_receiver) = mpsc::unbounded_channel::<MouseEvent>();

	// Create a join set to contain subordinate tasks.
	let join_set = Rc::new(RefCell::new(JoinSet::<Result<(), Error>>::new()));

	// Add a task to handle events.
	join_set
		.borrow_mut()
		.spawn_local(handle_events(event_receiver, display_connection));

	// Add tasks to watch for the termination signals. These tasks, and only these tasks, will
	// return Ok(true), and will do so once they receive their signal or once told to via the
	// CancellationToken.
	for i in signals {
		join_set
			.borrow_mut()
			.spawn_local(monitor_signal(i, cancel.clone()));
	}

	// Add tasks to accept connections from the listeners. These tasks will run until told to shut
	// down (via the CancellationToken), then return Ok(false).
	for i in tcp_listeners {
		let js = join_set.clone();
		join_set.borrow_mut().spawn_local(monitor_listener(
			i,
			cancel.clone(),
			js,
			event_sender.clone(),
			settings.clone(),
		));
	}
	for i in unix_listeners {
		let js = join_set.clone();
		join_set.borrow_mut().spawn_local(monitor_listener(
			i,
			cancel.clone(),
			js,
			event_sender.clone(),
			settings.clone(),
		));
	}

	// Drop the event sender so that only the listeners (and the tasks spawned by them) have copies
	// of them, so that once all the listeners and communication tasks end, the channel will close.
	drop(event_sender);

	// Run the application until shutdown or a fatal error occurs. Shutdown is indicated by a task
	// returning Ok(true). A fatal error is indicated by a task returning Err (non-fatal errors are
	// logged within the task and either do not result in termination or result in an Ok return
	// value).
	loop {
		select! {
			_ = cancel.cancelled() => break,
			result = JoinSetPoller::new(&join_set) => match result {
				Some(Ok(Ok(()))) => {
					// A task terminated normally.
				}
				Some(Ok(Err(e))) => {
					// A task terminated and returned Err(e). Tasks should only return Err in case
					// of fatal problems. Abort as quickly as possible.
					return Err(e);
				}
				Some(Err(e)) => {
					// A task panicked or was cancelled. Abort as quickly as possible.
					return Err(e.into());
				}
				None => {
					// There are no tasks left alive. This should never happen, because the signal
					// tasks should only ever terminate by returning Ok(true), at which point we
					// should break out of this loop before reaching this point.
					panic!("JoinSet became unexpectedly empty");
				}
			}
		}
	}

	// Wait up to five seconds for all the tasks to terminate before aborting them.
	log::debug!("Shutting down");
	let time_limit = tokio::time::Instant::now() + Duration::from_secs(5);
	loop {
		select! {
			result = JoinSetPoller::new(&join_set) => {
				match result {
					Some(Ok(Ok(()))) => {
						// A task terminated normally.
					}
					Some(Ok(Err(e))) => {
						// A task terminated and returned Err(e). Tasks should only return Err in
						// case of fatal problems. Abort as quickly as possible.
						return Err(e);
					}
					Some(Err(e)) => {
						// A task panicked or was cancelled. Abort as quickly as possible.
						return Err(e.into());
					}
					None => {
						// There are no tasks left.
						break;
					}
				}
			}
			_ = tokio::time::timeout_at(time_limit, futures_util::future::pending::<()>()) => {
				// The time limit has been reached and some tasks have failed to terminate. Return
				// an error, which, due to dropping the JoinSet, will forcibly abort them.
				return Err(Error::msg("Timed out waiting for tasks to terminate"));
			}
		}
	}

	log::debug!("Shutdown done");
	Ok(())
}
