use anyhow::{Context as _, Error};
use std::io::IoSlice;
use x11rb::connection::RequestConnection;
use x11rb::protocol::xproto::query_extension;
use x11rb::rust_connection::RustConnection;

/// A connection to the X11 display.
pub struct Connection {
	/// The connection.
	connection: RustConnection,

	/// The major opcode of the XTEST extension.
	xtest_opcode: u8,
}

impl Connection {
	/// Connects to the display.
	pub fn new(display: Option<&str>) -> Result<Self, Error> {
		let (connection, _) = x11rb::connect(display).context("Error connecting to X11 server")?;
		let query = query_extension(&connection, b"XTEST")
			.context("X11 communication error")?
			.reply()
			.context("Error querying XTEST extension")?;
		if !query.present {
			return Err(Error::msg("X11 server does not support XTEST extension"));
		}
		Ok(Self {
			connection,
			xtest_opcode: query.major_opcode,
		})
	}

	/// Sends a button click.
	pub fn click(&mut self) -> Result<(), Error> {
		self.send_xtest_event(x11rb::protocol::xproto::BUTTON_PRESS_EVENT, 0)?;
		self.send_xtest_event(x11rb::protocol::xproto::BUTTON_RELEASE_EVENT, 20)?;
		Ok(())
	}

	/// Sends an XTEST button event.
	fn send_xtest_event(&mut self, event: u8, time: u32) -> Result<(), Error> {
		let req = x11rb::protocol::xtest::FakeInputRequest {
			type_: event,
			detail: 1,
			time,
			root: 0,
			root_x: 0,
			root_y: 0,
			deviceid: 0,
		};
		let (bytes, fds) = req.serialize(self.xtest_opcode);
		let slices = bytes.iter().map(|b| IoSlice::new(b)).collect::<Vec<_>>();
		self.connection
			.send_request_without_reply(&slices, fds)?
			.check()?;
		Ok(())
	}

	/// Sends a pointer warp.
	pub fn warp(&mut self, x: i16, y: i16) -> Result<(), Error> {
		x11rb::protocol::xproto::warp_pointer(&self.connection, 0_u32, 0_u32, 0, 0, 0, 0, x, y)?
			.check()?;
		Ok(())
	}
}
