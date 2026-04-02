// test_features.js

os.log("--- Starting OS Feature Verification ---");

// 1. Test os.clipboard
os.log("Testing os.clipboard...");
os.clipboard.write("Hello from JSOS Verification!");
let clip = os.clipboard.read();
os.log("Clipboard read: '" + clip + "'");
if (clip === "Hello from JSOS Verification!") {
    os.log("SUCCESS: os.clipboard.write/read passed");
} else {
    os.log("FAILURE: os.clipboard mismatch");
}

// 2. Test os.websocket
os.log("Testing os.websocket...");
// Use a public echo server
let ws = os.websocket.connect("ws://echo.websocket.org");
os.log("Connecting to ws://echo.websocket.org, handle: " + ws);

let iterations = 0;
let connected = false;
let timeout = 50000; // Loop iterations for "async" wait

while (iterations < timeout) {
    let state = os.websocket.state(ws);
    if (state === "open") {
        if (!connected) {
            os.log("WebSocket connected!");
            os.websocket.send(ws, "Echo this!");
            connected = true;
        }
        let msg = os.websocket.recv(ws);
        if (msg) {
            os.log("WebSocket received: '" + msg + "'");
            if (msg === "Echo this!") {
                os.log("SUCCESS: os.websocket echo test passed");
            } else {
                os.log("FAILURE: os.websocket message mismatch");
            }
            os.websocket.close(ws);
            break;
        }
    } else if (state === "closed") {
        if (connected) {
            os.log("WebSocket closed normally.");
        } else {
            os.log("WebSocket failed to connect or closed unexpectedly.");
        }
        break;
    }
    iterations++;
}

if (iterations >= timeout) {
    os.log("TIMEOUT: WebSocket test timed out");
}

os.log("--- Verification Finished ---");
