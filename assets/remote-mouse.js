// The time between polls to check whether it’s time to send any accumulated
// motion.
const POLL_TIME = 10;

// How long to wait after an error that permits reconnecting before trying to
// reconnect.
const RECONNECT_TIME = 1000;

// The name of the subprotocol.
const SUBPROTOCOL = "ca.chead.remote-mouse.packets";

// The application state and functionality.
class Application {
	// The dialog box that should be shown when work is being done and the
	// application is not ready to use.
	#workingDialog = document.getElementById("working-dialog");

	// The text field in the working dialog box.
	#workingText = document.getElementById("working-text");

	// The progress bar in the working dialog box.
	#workingProgress = this.#workingDialog.getElementsByTagName("progress")[0];

	// The WebSocket URL.
	#url = (() => {
		const url = new URL("ws", document.URL);
		if(url.protocol.toLowerCase() === "https:") {
			url.protocol = "wss";
		} else {
			url.protocol = "ws";
		}
		return url;
	})();

	// The accumulated motion.
	#delta = [0, 0];

	// For each pointer that is down, the screen coordinates at which it was
	// last seen moving or being pressed down.
	#lastLocations = new Map();

	// Whether a timeout is pending to consider sending accumulated motion.
	#motionTimeoutPending = false;

	// The WebSocket.
	#socket = null;

	// Whether the socket is open (i.e. has received an “open” event and has
	// not received a “close” event).
	#open = false;

	// Starts the application.
	constructor() {
		// Hook up the input event handlers.
		const target = document.getElementById("target");
		target.addEventListener("click", this.#inputClick.bind(this));
		target.addEventListener("pointerenter", this.#inputPointerEnter.bind(this));
		target.addEventListener("pointermove", this.#inputPointerMove.bind(this));
		target.addEventListener("pointerout", this.#inputPointerOut.bind(this));

		// Connect the WebSocket.
		this.#connect();
	}

	// Starts connecting.
	#connect() {
		// Report what we’re doing.
		this.#workingText.replaceChildren("Connecting");
		this.#workingDialog.showModal();

		// Start connecting.
		this.#socket = new WebSocket(this.#url, SUBPROTOCOL);
		this.#socket.addEventListener("close", this.#socketClosed.bind(this));
		this.#socket.addEventListener("message", this.#socketMessage.bind(this));
		this.#socket.addEventListener("open", this.#socketOpen.bind(this));
	}

	// Handles the socket being closed.
	#socketClosed(e) {
		// Record the closure.
		const wasOpen = this.#open;
		this.#open = false;

		// Report the situation.
		this.#workingText.replaceChildren(`Connection closed: ${e.code} ${e.reason}.`);
		if(!this.#workingDialog.open) {
			this.#workingDialog.showModal();
		}

		// Close the socket in the other direction.
		this.#socket.close();
		let retry = false;
		switch(e.code) {
			case 1001: // Going Away
				this.#workingText.replaceChildren("Server offline.");
				break;

			case 1006: // Abnormal Closure
				this.#workingText.replaceChildren("Abnormal socket closure.");
				// This can happen during the connection attempt (i.e. before
				// “open”) if there is a problem with the handshake, in which
				// case the error is non-retryable. However, at least on
				// Firefox for Android, it can also happen if the browser is
				// suspended (e.g. due to a locked screen), in which case that
				// will typically happen after the connection is open and
				// should be retryable. A legitimate server will never
				// intentionally close the connection this way (it should send
				// a proper close frame instead), so an abnormal closure
				// post-open must indicate either the aforementioned
				// browser-side closure or else a catastrophic problem with the
				// server; in the latter case, we’ll try another connection
				// which will fail pre-open and then we’ll stop retrying.
				retry = wasOpen;
				break;

			case 1012: // Service Restart
				this.#workingText.replaceChildren("Server restarting.");
				retry = true;
				break;

			case 1013: // Try Again Later
				this.#workingText.replaceChildren("Server overloaded.");
				retry = true;
				break;
		}
		if(retry) {
			this.#workingText.append(" Retrying.");
			setTimeout(this.#connect.bind(this), RECONNECT_TIME);
		} else {
			this.#workingText.append(" Reload page to retry.");
			this.#workingProgress.hidden = true;
		}
	}

	// Handles a message arriving on the socket.
	#socketMessage(e) {
		// The server should never send us data.
		this.#socket.close(4000, "No messages expected.");
		this.#open = false;
	}

	// Handles the socket finishing opening.
	#socketOpen(e) {
		// Remember that we are open.
		this.#open = true;

		// Discard any accumulated delta and close the dialog box, letting the
		// user use the application.
		this.#delta[0] = 0;
		this.#delta[1] = 0;
		this.#workingDialog.close();
	}

	// Handles a “click” (mouse click or touch tap).
	#inputClick(e) {
		this.#flushMotion();
		this.#sendClick();
	}

	// Handles a “pointerenter”.
	#inputPointerEnter(e) {
		// Remember the location of the press as this pointer’s last location.
		this.#lastLocations.set(e.pointerId, [e.screenX, e.screenY]);
	}

	// Handles a “pointermove”.
	#inputPointerMove(e) {
		const last = this.#lastLocations.get(e.pointerId);
		const current = [e.screenX, e.screenY];
		this.#lastLocations.set(e.pointerId, current);
		// Sometimes we get moves without an enter. Just remember the location;
		// we’ll lose this pixel of movement but having the location recorded
		// means we can track in future.
		if(last !== undefined) {
			// Add the movement distance to the accumulated delta.
			this.#delta[0] += current[0] - last[0];
			this.#delta[1] += current[1] - last[1];

			// Decide whether to send the motion now or later.
			this.#maybeFlushMotion();
		}
	}

	// Handles a “pointerout”.
	#inputPointerOut(e) {
		// Forget this pointer’s last location because it is no longer
		// touching.
		this.#lastLocations.delete(e.pointerId);
	}

	// Handles a buffer-polling timeout.
	#timeout() {
		this.#motionTimeoutPending = false;
		this.#maybeFlushMotion();
	}

	// Checks whether the socket buffer is empty and either sends the
	// accumulated motion delta or starts another timeout to check later.
	#maybeFlushMotion() {
		if(this.#socket.bufferedAmount === 0) {
			// Send now.
			this.#flushMotion();
		} else if(this.#delta[0] !== 0 || this.#delta[1] !== 0) {
			// Socket send buffer is full; send later to avoid overfilling the
			// buffer and hopefully coalesce with some more motions.
			if(!this.#motionTimeoutPending) {
				setTimeout(this.#maybeFlushMotion.bind(this), POLL_TIME);
				this.#motionTimeoutPending = true;
			}
		}
	}

	// Sends any accumulated motion delta immediately.
	#flushMotion() {
		if(this.#delta[0] !== 0 || this.#delta[1] !== 0) {
			this.#sendMotion(this.#delta[0], this.#delta[1]);
			this.#delta[0] = 0;
			this.#delta[1] = 0;
		}
	}

	// Packs and sends a click message.
	#sendClick() {
		this.#sendPacket(0, 0, 0);
	}

	// Packs and sends a motion message.
	#sendMotion(x, y) {
		this.#sendPacket(1, x, y);
	}

	// Packs and sends a message, given its opcode and two parameters.
	#sendPacket(opcode, param1, param2) {
		if(this.#open) {
			const buffer = new ArrayBuffer(6);
			const view = new DataView(buffer);
			view.setUint16(0, opcode);
			view.setInt16(2, param1);
			view.setInt16(4, param2);
			this.#socket.send(buffer);
		}
	}
}

// Start the application.
new Application();
