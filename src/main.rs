//! Remote-Mouse provides a bridge between a WebSocket endpoint and an X11 display server, as well
//! as a basic web page that connects to the WebSocket endpoint and passes input gestures from the
//! browser to the endpoint allowing remote control of the mouse on the X11 server.

#![warn(
	// Turn on extra language lints.
	future_incompatible,
	missing_abi,
	nonstandard_style,
	rust_2018_idioms,
	single_use_lifetimes,
	trivial_casts,
	trivial_numeric_casts,
	unused,
	unused_crate_dependencies,
	unused_import_braces,
	unused_lifetimes,
	unused_qualifications,

	// Turn on extra Rustdoc lints.
	rustdoc::all,

	// Turn on extra Clippy lints.
	clippy::cargo,
	clippy::pedantic,
)]
// Nope, tabs thanks.
#![allow(clippy::tabs_in_doc_comments)]

mod app;
mod display;
mod systemd;

use anyhow::Context as _;
use clap::{CommandFactory as _, FromArgMatches as _, Parser};
use log::debug;
use std::ffi::c_int;
use std::net::ToSocketAddrs as _;
use std::os::fd::{FromRawFd as _, IntoRawFd as _};
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::{TcpListener, UnixListener};

/// A file permission bitmask.
#[derive(Clone, Copy, Debug, Hash, Eq, Ord, PartialEq, PartialOrd)]
struct Permissions(libc::mode_t);

impl std::fmt::Display for Permissions {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
		write!(f, "{:03o}", self.0)
	}
}

impl std::str::FromStr for Permissions {
	type Err = anyhow::Error;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let integer = libc::mode_t::from_str_radix(s, 8)?;
		if integer <= 0o777 {
			Ok(Self(integer))
		} else {
			Err(anyhow::Error::msg(format!(
				"Invalid file permissions {integer:o}, expected ≤777"
			)))
		}
	}
}

/// Returns the process’s current umask.
fn get_umask() -> libc::mode_t {
	// SAFETY:
	// - 0o777 is the most secure possible umask to temporarily apply.
	// - umask() cannot fail.
	// - This application is single-threaded, so the temporary umask change cannot affect other
	//   concurrently executing threads.
	// - There are no await points in this function, so the temporary umask change cannot affect
	//   other concurrently executing tasks.
	let value = unsafe { libc::umask(0o777) };
	unsafe { libc::umask(value) };
	value
}

/// The command-line parameters.
#[derive(Clone, Debug, Parser)]
#[command(about)]
#[command(
	after_help = "Mouse events received via WebSocket will be forwarded to the X11 display given in $DISPLAY.\nIn addition to those specified on the command line, any listening sockets passed via systemd-style socket passing will also be listened on."
)]
#[command(version)]
struct Args {
	/// The listen backlog for incoming WebSocket connections.
	#[arg(allow_negative_numbers = false)]
	#[arg(default_value_t = 32)]
	#[arg(long)]
	backlog: c_int,

	/// The X11 display to control.
	#[arg(long)]
	display: Option<String>,

	/// A TCP IPv4 or IPv6 host:port pair to listen on.
	#[arg(long)]
	listen_tcp: Vec<String>,

	/// A UNIX-domain socket filename to listen on.
	#[arg(long)]
	listen_unix: Vec<PathBuf>,

	/// A permitted origin (typically of the form “https://hostname”, the location where the HTML
	/// document that accesses the WebSocket connection is hosted) for incoming connections
	/// [default: accept from any origin].
	#[arg(long)]
	origin: Vec<String>,

	/// How long, in seconds, the connection must be idle before a ping is sent, or zero to never
	/// send pings.
	#[arg(default_value_t = 0)]
	#[arg(long)]
	ping_time: u32,

	/// The file permissions to apply to UNIX-domain listening sockets.
	#[arg(default_value_t = Permissions((0o777 ^ get_umask()) & 0o666))]
	#[arg(long)]
	unix_permissions: Permissions,
}

/// The top-level asynchronous future.
async fn async_main(
	args: Args,
	passed_sockets: std::ops::Range<std::os::fd::RawFd>,
	display_connection: display::Connection,
) -> Result<(), anyhow::Error> {
	// Turn any passed sockets into listener objects. The capacities may be overestimates because
	// each of passed_sockets is either TCP or UNIX-domain but not both, but that’s negligible.
	let mut tcp_listeners = Vec::with_capacity(args.listen_tcp.len() + passed_sockets.len());
	let mut unix_listeners = Vec::with_capacity(args.listen_unix.len() + passed_sockets.len());
	for i in passed_sockets {
		// SAFETY: We have already (in main) verified that all these FD numbers refer to existent
		// sockets of acceptable domains. We have not subsequently closed any of them. Therefore,
		// they must still be open. Therefore, they cannot have been substituted for other
		// subsequently opened FDs.
		let socket = unsafe { socket2::Socket::from_raw_fd(i) };
		match socket.domain()? {
			socket2::Domain::IPV4 | socket2::Domain::IPV6 => {
				tcp_listeners.push(TcpListener::from_std(socket.into())?);
			}
			socket2::Domain::UNIX => unix_listeners.push(UnixListener::from_std(socket.into())?),
			_ => panic!("Incorrect socket domain (but passed earlier domain check)"),
		}
	}

	// Create listeners.
	for i in args.listen_tcp {
		for j in i
			.to_socket_addrs()
			.with_context(|| format!("Error parsing {i} as TCP host:port"))?
		{
			let socket =
				socket2::Socket::new(socket2::Domain::for_address(j), socket2::Type::STREAM, None)?;
			socket.set_reuse_address(true)?;
			socket.set_nonblocking(true)?;
			socket
				.bind(&j.into())
				.with_context(|| format!("Error binding TCP socket at {j}"))?;
			socket.listen(args.backlog)?;
			tcp_listeners.push(TcpListener::from_std(socket.into())?);
		}
	}
	// SAFETY:
	// - By construction of args.unix_permissions, only the bottom 9 bits may be set.
	// - umask() never fails.
	// - We have not spawned any additional tasks, and also there are no await points within the
	//   loop, so the umask change will not affect any other concurrently executing code.
	let old_umask = unsafe { libc::umask(0o777 ^ args.unix_permissions.0) };
	for i in args.listen_unix {
		let _ = std::fs::remove_file(&i);
		unix_listeners.push(
			UnixListener::bind(&i)
				.with_context(|| format!("Error binding UNIX socket at {}", i.display()))?,
		);
	}
	// SAFETY:
	// - old_mask was returned by a previous umask() call.
	// - umask() never fails.
	// - We have not spawned any additional tasks, and also there are no await points within the
	//   loop, so the umask change will not affect any other concurrently executing code.
	unsafe { libc::umask(old_umask) };
	debug!(
		"Obtained {} TCP and {} UNIX-domain listeners",
		tcp_listeners.len(),
		unix_listeners.len()
	);

	// Get notifications of termination signals.
	let signals = [
		tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
		tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
	];

	// Run the application in a proper task (the Tokio documentation says that the top-level
	// blocked-on future is not as efficient as spawned tasks, so avoid doing significant runtime
	// work in it).
	tokio::task::spawn_local(app::run(
		display_connection,
		signals,
		tcp_listeners,
		unix_listeners,
		args.origin,
		Duration::from_secs(args.ping_time.into()),
	))
	.await?
}

fn main() -> Result<(), anyhow::Error> {
	// Initialize logging.
	env_logger::init();

	// Figure out how many sockets were passed in systemd style.
	let passed_sockets = systemd::passed_sockets()?;
	debug!(
		"Received {} systemd-style passed sockets",
		passed_sockets.len()
	);

	// Verify that all the passed-in sockets actually exist, are sockets, and are set up properly.
	// Doing this now ensures that the invoker can’t have left a hole in the FD space, which future
	// code (after these checks) could fill by opening a file and then have that new FD
	// misinterpreted as being a passed-in socket.
	for i in passed_sockets.clone() {
		// SAFETY:
		// - We can’t accidentally refer to stdin/stdout/stderr, because PassedSockets promises not
		//   to return those descriptor numbers.
		// - If a particular socket was not passed at all (i.e. the relevant FD is closed), we will
		//   discover that when we try to examine it. We can’t accidentally refer to some other
		//   descriptor that has been opened since start and has taken up that slot, because we
		//   haven’t opened any new descriptors yet.
		// - If a particular FD is not a socket, or is a socket of the wrong domain, we will safely
		//   discover that when we inquire as to the domain and the kernel returns an error.
		let socket = unsafe { socket2::Socket::from_raw_fd(i) };
		let domain = socket.domain()?;
		if !socket.is_listener()? {
			return Err(anyhow::Error::msg(
				"A systemd-style passed socket was not listening",
			));
		}
		if !socket.nonblocking()? {
			return Err(anyhow::Error::msg(
				"A systemd-style passed socket was in blocking mode",
			));
		}
		match domain {
			socket2::Domain::IPV4 | socket2::Domain::IPV6 | socket2::Domain::UNIX => (),
			_ => {
				return Err(anyhow::Error::msg(
					"A systemd-style passed socket was not IPv4, IPv6, or UNIX-domain",
				))
			}
		}
		// Turn it back into a raw FD so that dropping the Socket object won’t close it. Don’t do
		// anything with the result; we will discover again it by iteration later when we’re ready
		// to actually create listeners.
		socket.into_raw_fd();
	}

	// Parse command line parameters. If no sockets were passed, then at least one listen address
	// must be passed on the command line.
	let command = Args::command();
	let command = if passed_sockets.is_empty() {
		command
			.mut_arg("listen_tcp", |arg| {
				arg.required_unless_present("listen_unix")
			})
			.mut_arg("listen_unix", |arg| {
				arg.required_unless_present("listen_tcp")
			})
	} else {
		command
	};
	let args = Args::from_arg_matches(&command.get_matches())?;

	// Connect to the display.
	let display_connection = display::Connection::new(args.display.as_deref())?;

	// Start up a Tokio runtime.
	let runtime = tokio::runtime::Builder::new_current_thread()
		.enable_io()
		.enable_time()
		.build()?;
	tokio::task::LocalSet::new().block_on(
		&runtime,
		async_main(args, passed_sockets, display_connection),
	)
}
