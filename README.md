Overview
========

Remote-Mouse provides a bridge between a WebSocket endpoint and an X11 display
server, as well as a basic web page that connects to the WebSocket endpoint and
passes input gestures from the browser to the endpoint allowing remote control
of the mouse on the X11 server.


Configuration
=============

Remote-Mouse is meant to run as a backend behind a frontend HTTP server. As
such, Remote-Mouse doesn’t implement any authentication; that should be done by
the HTTP server, and Remote-Mouse itself should not be exposed to any untrusted
clients.

To set up Remote-Mouse, you’ll need to do the following:
1. Determine what hostname you’ll be using to connect (this checklist is
   written assuming `example.com`), whether you’ll use HTTP or HTTPS (this
   checklist is written assuming HTTPS), and a directory name within the server
   where you’ll host the application (this checklist is written assuming
   `/mouse/`, making a complete URL of `https://example.com/mouse/`).
2. Set up an HTTP server and make sure you can connect to it at your chosen URL
   (which at this point will probably return 404, but should connect
   successfully), establishing any needed DNS records, TLS certificates, etc..
3. Set up HTTP authentication, if desired, protecting your chosen directory.
4. Publish the contents of the `assets/` directory at your chosen URL, such
   that `https://example.com/mouse/` returns `index.html`,
   `https://example.com/mouse/remote-mouse.js` returns `remote-mouse.js`, and
   so on.
5. At the `/ws` location within your chosen directory (e.g.
   `https://example.com/mouse/ws`), set up an HTTP reverse proxy pointing at an
   endpoint where Remote-Mouse will listen (a UNIX-domain socket endpoint is
   recommended as it can be better secured so that only the HTTP server, and
   not other processes belonging to other users on the same computer, can
   connect to it, but a TCP `localhost` binding is also possible), configured
   if necessary to pass WebSocket connections using the `Upgrade` HTTP header.
6. Run the `remote-mouse` binary, passing the necessary option to listen at the
   UNIX-domain or TCP location chosen in step 5, plus any others as needed. If
   your HTTP server is configured to time out idle connections, you’ll want to
   set `--ping-time` to a shorter period to prevent the WebSocket connections
   from being closed. Also pass the `--origin` option with the origin portion
   of the chosen URL (e.g. `https://example.com`).
7. Navigate to your chosen URL in a Web browser, typically on another computer
   or a mobile device.


Usage
=====

In your Web browser, once the connection is established, the screen will be
blank. If using a mouse or similar input device, moving the mouse pointer
around the page will cause the mouse pointer on the server to move, and
left-clicking will send a left-click to the server. If using a touchscreen or
similar input device, swiping around the page will cause the mouse pointer on
the server to move, and tapping will send a left-click to the server.


Example Lighttpd configuration
==============================

This configuration is suitable for an HTTPS server, where
`/etc/lighttpd/mouse.passwd` contains the authentication credentials, and
`/run/remote-mouse/mouse.sock` is where Remote-Mouse is listening for incoming
proxied connections.

```
$HTTP["scheme"] == "https" {
	$HTTP["url"] =~ "^/mouse" {
		auth.backend = "plain"
		auth.backend.plain.userfile = "/etc/lighttpd/mouse.passwd"
		auth.require = ("" => ("method" => "basic", "realm" => "Remote mouse controller", "require" => "valid-user"))

		$HTTP["url"] == "/mouse/ws" {
			proxy.server = ("" => ("mouse" => (
				"socket" => "/run/remote-mouse/mouse.sock",
			)))
			proxy.header = (
				"upgrade" => "enable",
			)
		}
	}
}
```
