const EventEmitter = require('events');
const Buffer = require('buffer');

console.log("=== Testing Polyfills ===");

// Test EventEmitter
const map = new EventEmitter();
let msg = "";
map.on("greet", (name) => { msg = "Hello " + name; });
map.emit("greet", "JSOS");
console.log("EventEmitter emit: " + (msg === "Hello JSOS" ? "PASS" : "FAIL"));

// Test Buffer
const buf1 = Buffer.from("Buffer test");
const str = buf1.slice(0, 6).toString();
console.log("Buffer slice + toString: " + (str === "Buffer" ? "PASS" : "FAIL"));

os.exit();
