use std::io::{Error, ErrorKind};
use tokio::io::{AsyncBufRead, AsyncReadExt, AsyncWrite};

/// A close code indicating normal closure, meaning that the purpose for which the connection was
/// established has been fulfilled.
pub const CLOSE_NORMAL: u16 = 1000;

/// A close code indicating that an endpoint is “going away”, such as a server going down or a
/// browser having navigated away from the page.
pub const CLOSE_GOING_AWAY: u16 = 1001;

/// A close code indicating termination due to a protocol error.
///
/// This code is sent by the peer if it considers itself to have received a protocol violation. The
/// application should never initiate a close using this code, though the library will send it if
/// the peer violates protocol.
///
/// This code is only indicated as a received close code if the peer sends it, presumably in
/// response to the local endpoint violating protocol in a sent frame. If the library detects a
/// protocol violation in a received frame, this code is sent to the peer, but
/// [`CLOSE_RX_PROTOCOL_ERROR`](CLOSE_RX_PROTOCOL_ERROR) is indicated as the local receive close
/// code.
pub const CLOSE_PROTOCOL_ERROR: u16 = 1002;

/// A close code indicating that an endpoint is terminating the connection because it received a
/// type of data it cannot accept (e.g., an endpoint that understands only text data but received a
/// binary message).
pub const CLOSE_UNSUPPORTED_DATA: u16 = 1003;

/// A close code indicating that the peer closed the connection gracefully, but without specifying
/// a close code.
///
/// This code is never sent over the wire. The application should never initiate a close using this
/// code. The library will report it as a receive close code if a close frame was received without
/// a close code.
pub const CLOSE_NO_STATUS_RECEIVED: u16 = 1005;

/// A close code indicating that the peer closed the transport without sending a close frame.
///
/// This code is never sent over the wire. The application should never initiate a close using this
/// code. The library will report it as a receive close code it if a transport closure occurs
/// without a preceding close frame.
pub const CLOSE_ABNORMAL: u16 = 1006;

/// A close code indicating that an endpoint is terminating the connection because it received a
/// text message containing a non-UTF-8 payload.
///
/// This code is sent by the peer if it considers itself to have received a non-UTF-8 payload. The
/// application should never initiate a close using this code, though the library will send it if
/// the peer sends a non-UTF-8 text-type message.
///
/// This code is only indicated as a received close code if the peer sends it, presumably in
/// response to the local endpoint sending a non-UTF-8 text message. If the library detects a
/// non-UTF-8 payload in a received text message, this code is sent to the peer, but
/// [`CLOSE_RX_INVALID_FRAME_PAYLOAD_DATA`](CLOSE_RX_INVALID_FRAME_PAYLOAD_DATA) is indicated as
/// the local receive close code.
pub const CLOSE_INVALID_FRAME_PAYLOAD_DATA: u16 = 1007;

/// A close code indicating that an endpoint is terminating the connection because it received a
/// message that violates its policy.
pub const CLOSE_POLICY_VIOLATION: u16 = 1008;

/// A close code indicating that an endpoint is terminating the connection because it received a
/// message that is too big for it to process.
///
/// This code is sent by the peer if it considers itself to have received a too-big message. The
/// application may initiate a close using this code. The library will also send it if the peer
/// sends a message which exceeds the specified maximum message size.
///
/// This code is only reported as a received close code if the peer sends it, presumably in
/// response to the local endpoint sending a too-long message. If the library detects a too-long
/// received message, this code is sent to the peer, but
/// [`CLOSE_RX_MESSAGE_TOO_BIG`](CLOSE_RX_MESSAGE_TOO_BIG) is indicated as the local receive close
/// code.
pub const CLOSE_MESSAGE_TOO_BIG: u16 = 1009;

/// A close code indicating that the client is terminating the connection because it expected the
/// server to negotiate one or more extensions, but the server didn’t return them in the response
/// message of the WebSocket handshake.
pub const CLOSE_MANDATORY_EXTENSION: u16 = 1010;

/// A close code indicating that the server is terminating the connection because it encountered an
/// unexpected condition that prevented it from fulfilling the request.
pub const CLOSE_INTERNAL_SERVER_ERROR: u16 = 1011;

/// A close code indicating that the library detected a protocol violation from the peer.
///
/// This code is never sent over the wire. The application should never initiate a close using this
/// code. The library will report it as a receive close code if the peer violated protocol. If the
/// peer claims a protocol violation by the local endpoint,
/// [`CLOSE_PROTOCOL_ERROR`](CLOSE_PROTOCOL_ERROR) is reported as the receive close code instead.
pub const CLOSE_RX_PROTOCOL_ERROR: u16 = 4358;

/// A close code indicating that the library detected a non-UTF-8 payload in text message received
/// from the peer.
///
/// This code is never sent over the wire. The application should never initiate a close using this
/// code. The library will report it as a receive close code if the peer sent a non-UTF-8 text
/// message. If the peer claims a non-UTF-8 text message from the local endpoint,
/// [`CLOSE_INVALID_FRAME_PAYLOAD_DATA`](CLOSE_INVALID_FRAME_PAYLOAD_DATA) is reported as the
/// receive close code instead.
pub const CLOSE_RX_INVALID_FRAME_PAYLOAD_DATA = 4359;

/// A close code indicating that the library detected a too-long message received from the peer.
///
/// This code is never sent over the wire. The application should never initiate a close using this
/// code. The library will report it as a receive close code if the peer sent a too-long message.
/// If the peer claims a too-long message from the local endpoint,
/// [`CLOSE_MESSAGE_TOO_BIG`](CLOSE_MESSAGE_TOO_BIG) is reported as the receive close code instead.
pub const CLOSE_RX_MESSAGE_TOO_BIG = 4360;

/// An opcode that defines the type of a WebSocket frame.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Opcode {
	/// The frame contains application data that is appended to an existing, unfinished message.
	Continuation,

	/// The frame contains the start of a new text message.
	Text,

	/// The frame contains the start of a new binary message.
	Binary,

	/// The connection is being closed.
	Close,

	/// The recipient should reply with a [`PONG`](Opcode::Pong).
	Ping,

	/// The reply to a [`PING`](Opcode::Ping).
	Pong,
}

impl Opcode {
	/// Indicates whether this opcode is a control opcode.
	pub fn control(self) -> bool {
		match self {
			Opcode::Continuation | Opcode::Text | Opcode::Binary => false,
			Opcode::Close | Opcode::Ping | Opcode::Pong => true,
		}
	}
}

impl std::convert::TryFrom<u8> for Opcode {
	type Error = std::io::Error;

	fn try_from(raw: u8) -> Result<Self, Self::Error> {
		match raw {
			0x00 => Ok(Self::Continuation),
			0x01 => Ok(Self::Text),
			0x02 => Ok(Self::Binary),
			0x08 => Ok(Self::Close),
			0x09 => Ok(Self::Ping),
			0x0A => Ok(Self::Pong),
			_ => Err(ErrorKind::InvalidData.into()),
		}
	}
}

/// The header of a single frame.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrameHeader {
	/// Whether this frame is the last one in a message.
	///
	/// This will always be `true` for control frames.
	pub fin: bool,

	/// The opcode.
	pub opcode: Opcode,

	/// The length, in bytes, of the frame payload.
	pub length: u64,

	/// The mask to XOR with the frame payload.
	pub mask: [u8; 4],
}

impl FrameHeader {
	/// Reads one frame header.
	///
	/// On success, the reader is positioned at the start of the frame payload.
	///
	/// # Errors
	/// * [`UnexpectedEof`](ErrorKind::UnexpectedEof) if `src` reaches EOF.
	/// * [`InvalidData`](ErrorKind::InvalidData) if the peer sends non-compliant data.
	/// * Any error returned by `src`.
	pub async fn read(mut src: impl tokio::io::AsyncBufRead + std::marker::Unpin) -> Result<Self, Error> {
		// The first byte contains the FIN bit, three reserved bits, and four opcode bits.
		let first_byte = match src.read_u8().await?;
		let fin: bool = (first_byte & 0x80) != 0;
		let opcode: Opcode = (first_byte & 0x0F).try_into()?;

		// The reserved bits must not be set.
		if (first_byte & 0x70) != 0 {
			return Err(ErrorKind::InvalidData.into());
		}

		// A control message must never be fragmented, and therefore a frame with a control opcode
		// must always have the FIN bit set.
		if opcode.control() && !fin {
			return Err(ErrorKind::InvalidData.into());
		}

		// The second byte contains the MASK bit and the payload length (or indication of longer
		// length).
		let second_byte = src.read_u8().await?;
		let mask: bool = (second_byte & 0x80) != 0;
		let length: u64 = (second_byte & 0x7F).into();

		// All frames from client to server must be masked.
		if !mask {
			return Err(ErrorKind::InvalidData.into());
		}

		// A control message must always be ≤125 bytes long.
		if opcode.control() && length >= 126 {
			return Err(ErrorKind::InvalidData.into());
		}

		// Get the actual length, if it is encoded using more bytes.
		let length = match length {
			126 => {
				let length = u64::from(src.read_u16().await?);
				if length <= 125 {
					// A 16-bit form was used to encode a value that would have fit into a 7-bit
					// form. Overlong encodings are illegal.
					return Err(ErrorKind::InvalidData.into());
				}
				length
			}
			127 => {
				let length = src.read_u64().await?;
				if length <= 65535 {
					// A 64-bit form was used to encode a value that would have fit into a 16-bit
					// form. Overlong encodings are illegal.
					return Err(ErrorKind::InvalidData.into());
				}
				length
			}
			v => u64::from(v),
		};

		// A 64-bit length must not have its most significant bit set.
		if length >= 0x8000000000000000_u64 {
			return Err(ErrorKind::InvalidData.into());
		}

		// The mask value is the next four bytes.
		let mut mask = [0_u8; 4];
		src.read_exact(&mut mask).await?;

		Ok(Self {
			fin: (first_byte & 0x80) != 0,
			opcode: Opcode::try_from(first_byte & 0x0F)?,
			length,
			mask,
		})
	}
}

/// The two types of messages that can be sent or received over a WebSocket.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum MessageType {
	/// A UTF-8 string.
	Text,

	/// A binary blob.
	Binary,
}

/// A message that can be sent or received over a WebSocket.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Message<'a> {
	/// A UTF-8 string.
	Text(&'a str),

	/// A binary blob.
	Binary(&'a [u8]),
}

/// A WebSocket.
#[derive(Clone, Debug)]
struct Connection<R: AsyncBufRead, W: AsyncWrite> {
	/// The reading half.
	reader: R,

	/// The writing half.
	writer: W,

	/// A buffer to hold portions of a message until the message is consumed.
	buffer: Box<[u8]>,

	/// The received close code, if any, or `None` if neither a [`CLOSE`](Opcode::Close) frame, a
	/// transport-layer EOF, nor a protocol error has been received.
	rx_close_code: Option<u16>,

	/// Whether the transmit half has been closed.
	tx_closed: bool,
}

impl<R: AsyncBufRead, W: AsyncWrite> Connection<R, W> {
	/// Wraps a reader and writer which are speaking the WebSocket protocol.
	///
	/// It is expected that the handshake process is complete, and the socket is `OPEN`.
	pub fn new(reader: R, writer: W, max_message_size: usize) -> Self {
		Self {
			reader,
			writer,
			buffer: vec![0_u8; max_message_size].into_boxed_slice(),
			close_reason: None,
			tx_closed: false,
		}
	}

	/// Receives the next message from the socket.
	///
	/// If the peer has closed the socket, `None` is returned.
	pub async fn read(&'a mut self) -> Result<Option<Message<'a>>, Error> {
		// If the receive path has been closed, return immediately and don’t touch the socket.
		if self.rx_close_reason.is_some() {
			return Ok(None);
		}

		// Handle errors properly.
		match self.inner() {
			Ok(v) => Ok(v),
			Err(e) if e.kind() == ErrorKind::UnexpectedEof {
				// The peer closed the socket.
				self.rx_close_reason = Some(CLOSE_ABNORMAL);
				Ok(None)
			}
			Err(e) if e.kind() == ErrorKind::InvalidData {
				// The peer violated protocol rules.
				self.rx_close_reason = Some(CLOSE_RX_PROTOCOL_ERROR);
				Err(e)
			}
			Err(e) {
				// A transport error occurred.
				self.rx_close_reason = Some(CLOSE_ABNORMAL);
				Err(e)
			}
		}

		async fn inner(&'a mut self) -> Result<Option<Message<'a>>, Error> {
			// Receive until the message is complete.
			let mut message_type: Option<MessageType> = None;
			let mut bytes_received = 0_usize;
			loop {
				// Receive a frame.
				let header = FrameHeader::read(&mut self.reader).await?;

				// Deal with the frame.
				let buffer_payload = match header.opcode {
					Opcode::Continuation => {
						// A continuation frame is only legal if we already received a non-continuation
						// frame.
						if message_type.is_none() {
							return Err(ErrorKind::InvalidData.into());
						}
						true
					}
					Opcode::Text => {
						// A text frame is only legal if we have *not* already received any data
						// frames.
						if message_type.is_some() {
							return Err(ErrorKind::InvalidData.into());
						}
						message_type = Some(MessageType::Text);
						true
					}
					Opcode::Binary => {
						// A binary frame is only legal if we have *not* already received any data
						// frames.
						if message_type.is_some() {
							return Err(ErrorKind::InvalidData.into());
						}
						message_type = Some(MessageType::Binary);
						true
					}
					Opcode::Close => {
						match header.length {
							0 => {
								// No close code was included.
								self.rx_close_reason = Some(CLOSE_NO_STATUS_RECEIVED);
								return Ok(None);
							}
							1 => {
								// Half of a close code was included, which is illegal.
								return Err(ErrorKind::InvalidData.into());
							}
							n => {
								// The whole close code was included. Read it.
								let mut close_code = [0_u8; 2];
								self.read_and_unmask(&mut close_code).await?;

								// Read and discard the remainder of the frame, which holds the close
								// reason text.
								{
									let mut remaining = n - 2;
									while remaining != 0 {
										let current = self.reader.fill_buf().await?;
										if current.len() >= remaining {
											self.reader.consume(remaining as usize);
											remaining = 0;
										} else {
											self.reader.consume(current.len());
											remaining -= current.len();
										}
									}
								}

								// Sanity check that the close code is one that the peer is legally
								// allowed to send over the wire.
								todo!();
							}
						}
					}
					Opcode::Ping => {
						todo!();
					}
					Opcode::Pong => {
						todo!();
					}
				}

				// If this frame carries a payload that we are supposed to include in the message,
				// do that.
				todo!();

				// If this is the last frame in the message, we’re done now.
				todo!();
			}
		}
	}
}
