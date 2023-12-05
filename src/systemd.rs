//! APIs for interacting with systemd.

use anyhow::{Context as _, Error};
use log::trace;
use std::env::{var, VarError};
use std::ops::Range;
use std::os::fd::RawFd;
use std::str::FromStr as _;

/// The file descriptor number of the first socket passed in systemd socket-passing style.
const SD_LISTEN_FDS_START: RawFd = 3;

/// Obtains the range of passed sockets.
pub fn passed_sockets() -> Result<Range<RawFd>, Error> {
	const EMPTY: Range<RawFd> = Range { start: 0, end: 0 };

	// LISTEN_PID must exist and be equal to our own process ID.
	match var("LISTEN_PID") {
		Ok(s) => {
			let pid = u32::from_str(&s).context("Invalid $LISTEN_PID")?;
			if pid != std::process::id() {
				trace!("No systemd sockets because $LISTEN_PID does not match");
				return Ok(EMPTY);
			}
		}
		Err(VarError::NotPresent) => {
			trace!("No systemd sockets because $LISTEN_PID is missing");
			return Ok(EMPTY);
		}
		Err(e) => return Err(e).context("Invalid $LISTEN_PID"),
	}

	// LISTEN_FDS indicates how many sockets there are.
	let count = match var("LISTEN_FDS") {
		Ok(s) => RawFd::from_str(&s).context("Invalid $LISTEN_FDS")?,
		Err(VarError::NotPresent) => {
			trace!("No systemd sockets because $LISTEN_FDS is missing");
			return Ok(EMPTY);
		}
		Err(e) => return Err(e).context("Invalid $LISTEN_FDS"),
	};
	if count < 0 {
		return Err(Error::msg("Invalid $LISTEN_FDS: cannot be negative"));
	}
	let Some(end) = count.checked_add(SD_LISTEN_FDS_START) else {
		return Err(Error::msg("Invalid $LISTEN_FDS: too large"));
	};

	Ok(Range {
		start: SD_LISTEN_FDS_START,
		end,
	})
}
