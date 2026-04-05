# TFTP/FTP/SFTP Protocol Support & PXE Config Analysis

**Date:** 2026-04-05
**Status:** Approved

## Overview

Add TFTP, FTP, and SFTP client protocol support to JSOS via a standalone `no_std` protocol crate. Extract existing HTTP/WebSocket protocol logic into the same crate. Expose all protocols to JavaScript apps through `os.*` APIs with JSKV storage integration. Build a PXE configuration analysis library and tool for inspecting/auditing/deploying Windows PXE boot setups.

## 1. Protocol Crate (`protocols/`)

A new `no_std + alloc` crate at the repository root. Contains pure protocol state machines with no socket or smoltcp dependency. Each protocol produces bytes to send and consumes bytes received — the kernel owns sockets and feeds data in/out.

### Crate Structure

```
protocols/
  Cargo.toml          # no_std, alloc, no smoltcp dependency
  src/
    lib.rs            # re-exports
    tftp.rs           # TFTP client (RFC 1350, UDP)
    ftp.rs            # FTP client (RFC 959, TCP control+data)
    sftp.rs           # SFTP client (SSH file transfer, TCP)
    http.rs           # HTTP/1.1 request/response (extracted from src/net/mod.rs)
    websocket.rs      # WebSocket framing RFC 6455 (extracted from src/net/mod.rs)
```

### State Machine Interface Pattern

Each protocol exposes a state machine that communicates via action enums. The kernel integration layer interprets these actions and drives the sockets.

```rust
// Example: TFTP
pub enum TftpAction {
    Send(Vec<u8>),          // send these bytes on the UDP socket
    Complete(Vec<u8>),      // transfer done, here is the file data
    Error(TftpError),       // protocol error
}

pub struct TftpClient { /* internal state */ }

impl TftpClient {
    /// Start a read request. Returns (client, initial RRQ packet to send).
    pub fn start_read(server: Ipv4Addr, filename: &str) -> (Self, Vec<u8>);

    /// Start a write request. Returns (client, initial WRQ packet to send).
    pub fn start_write(server: Ipv4Addr, filename: &str, data: &[u8]) -> (Self, Vec<u8>);

    /// Feed received UDP data, get next action.
    pub fn receive(&mut self, data: &[u8]) -> TftpAction;
}
```

FTP and SFTP follow the same pattern but with session-oriented state (connect, authenticate, then multiple operations).

### Protocol Details

**TFTP (RFC 1350):**
- UDP port 69 (initial), ephemeral ports for transfer
- Opcodes: RRQ (1), WRQ (2), DATA (3), ACK (4), ERROR (5)
- 512-byte block size (standard), octet transfer mode
- Simple stop-and-wait acknowledgment

**FTP (RFC 959):**
- TCP port 21 (control channel)
- Passive mode (PASV) for data channel — avoids NAT issues
- Commands: USER, PASS, LIST, RETR, STOR, MKD, DELE, QUIT, TYPE, PASV
- Text-based command/response protocol

**SFTP (SSH File Transfer Protocol):**
- Runs over SSH (TCP port 22)
- Requires SSH transport layer (key exchange, encryption)
- Binary packet protocol with request/response IDs
- Commands: OPEN, READ, WRITE, STAT, OPENDIR, READDIR, CLOSE, REMOVE, MKDIR

**HTTP/1.1 (extracted from existing code):**
- Request building (method, headers, body)
- Response parsing (status, headers, chunked/content-length body)
- Redirect handling

**WebSocket (RFC 6455, extracted from existing code):**
- Upgrade handshake with Sec-WebSocket-Key
- Frame encoding/decoding (text, binary, ping, pong, close)
- Masking for client frames

## 2. Kernel Integration Layer (`src/net/`)

New files in `src/net/` that glue protocol state machines to smoltcp sockets.

### New Files

```
src/net/
  tftp_job.rs       # TftpJob: UDP socket <-> TftpClient
  ftp_job.rs        # FtpJob: TCP sockets <-> FtpClient
  sftp_job.rs       # SftpJob: TCP socket <-> SftpClient
```

### Job Structures

Each job owns a smoltcp socket handle and a protocol state machine instance:

```rust
pub struct TftpJob {
    pub pid: usize,
    pub udp_handle: SocketHandle,
    pub client: TftpClient,
    pub store_key: String,       // JSKV key to store result
    pub started_at: u64,
}

pub struct FtpSession {
    pub pid: usize,
    pub session_id: u64,
    pub control_handle: SocketHandle,
    pub data_handle: Option<SocketHandle>,
    pub client: FtpClient,
}

pub struct SftpSession {
    pub pid: usize,
    pub session_id: u64,
    pub tcp_handle: SocketHandle,
    pub client: SftpClient,
}
```

### Polling

Each job type gets a poll function called from `poll_network()`:

- `poll_tftp_jobs()` — read UDP data, feed to `TftpClient::receive()`, handle actions (send, complete, error), write completed files to JSKV
- `poll_ftp_sessions()` — read TCP control/data channels, feed to `FtpClient`, handle multi-step commands
- `poll_sftp_sessions()` — read TCP data, feed to `SftpClient`, handle SSH transport + SFTP commands

### Refactoring `mod.rs`

Existing HTTP fetch and WebSocket logic moves from inline code in `mod.rs` to using the `protocols` crate's `HttpClient` and `WsClient` state machines. `FetchJob` and `WebSocketJob` structs remain in `mod.rs` but their protocol logic delegates to the crate. This significantly reduces the size of `mod.rs` (~1000+ lines of protocol parsing moves to the crate).

### Global State

New global collections alongside existing ones:

```rust
static TFTP_JOBS: Mutex<Vec<TftpJob>> = Mutex::new(Vec::new());
static FTP_SESSIONS: Mutex<BTreeMap<u64, FtpSession>> = Mutex::new(BTreeMap::new());
static SFTP_SESSIONS: Mutex<BTreeMap<u64, SftpSession>> = Mutex::new(BTreeMap::new());
```

Process cleanup (`cleanup_process_network`) extended to close TFTP/FTP/SFTP resources on process exit.

## 3. JavaScript API

### TFTP

```js
// Download file from TFTP server, save to JSKV store key
os.tftp.get("192.168.1.1", "/boot/BCD", "pxe:bcd")

// Upload from JSKV store key to TFTP server
os.tftp.put("192.168.1.1", "/path/file", "pxe:bcd")
```

Both return Promises. TFTP is stateless — no connection object.

### FTP

```js
let ftp = os.ftp.connect("192.168.1.1", { user: "admin", pass: "secret" })

ftp.list("/")                              // Promise<string> - directory listing
ftp.get("/boot/pxeboot.n12", "pxe:boot")  // Promise - download to store key
ftp.put("/remote/path", "pxe:boot")        // Promise - upload from store key
ftp.mkdir("/newdir")                        // Promise
ftp.delete("/file")                         // Promise
ftp.close()                                 // close session
```

Session-based. Connection object tracks the control channel. Data transfers use PASV mode.

### SFTP

```js
let sftp = os.sftp.connect("192.168.1.1", { user: "admin", pass: "secret" })

sftp.list("/")                             // Promise<string> - directory listing
sftp.get("/path", "local:key")             // Promise - download to store key
sftp.put("/remote/path", "local:key")      // Promise - upload from store key
sftp.stat("/path")                         // Promise<{size, modified, perms}>
sftp.close()
```

Same session pattern as FTP.

### Storage Convention

All local file references are JSKV store keys using `:` as the separator (not `/`). Examples: `pxe:bcd`, `pxe:boot:menu`, `configs:wds:main`. Remote paths on TFTP/FTP/SFTP servers use whatever separator the server expects (typically `/`).

### HTTP / WebSocket

`os.fetch()` and WebSocket APIs remain unchanged externally. Internal implementation refactored to use the `protocols` crate.

## 4. PXE Configuration Analysis

### Library: `src/js/pxe.js`

Importable library for parsing and analyzing Windows PXE boot configurations.

**Capabilities:**
- **BCD parsing** — decode Boot Configuration Data binary format, extract boot entries, OS loader paths, boot menu text, timeout values
- **Config inspection** — display WDS server settings, TFTP paths, image catalog
- **Audit checks** — flag security issues:
  - Unsigned boot images
  - Open/unauthenticated access
  - Rogue server detection (compare expected vs actual TFTP server)
  - Misconfigured DHCP options
- **Config modification** — modify BCD entries, boot menu text, image paths for redeployment
- **Export** — serialize modified configs back to binary for upload

### App: `src/jsos/pxetool.jsos`

Interactive tool built on `pxe.js`:

```
pxetool scan 192.168.1.0/24    # discover TFTP/WDS servers on network
pxetool pull 192.168.1.1       # fetch all PXE config files from server
pxetool show pxe:bcd           # parse and display BCD store contents
pxetool audit pxe:bcd          # run security audit checks
pxetool edit pxe:bcd           # modify config entries
pxetool deploy 192.168.1.1     # upload modified configs back to server
```

## 5. Scope Boundaries

### In Scope
- TFTP, FTP, SFTP client protocol state machines in `protocols/` crate
- HTTP and WebSocket extraction into `protocols/` crate
- Kernel socket glue in `src/net/` job files
- JS API bindings for all protocols
- JSKV storage integration
- PXE library (`src/js/pxe.js`) and tool (`src/jsos/pxetool.jsos`)

### Out of Scope (for now)
- NIC drivers for bare metal hardware (user working on this separately)
- RamFS integration (JSKV only)
- DHCP option 66/67 auto-discovery
- IPv6 support
- FTP active mode (PASV only)
- SSH key-based authentication for SFTP (password only initially)

## 6. Dependencies

### Protocol Crate
- `alloc` (already available in kernel)
- No new external crates for TFTP/FTP (simple enough to implement directly)
- SFTP will need SSH transport — evaluate whether to implement minimal SSH or use an existing `no_std` crate

### Kernel
- smoltcp `socket-udp` feature (already enabled, currently unused)
- Existing `embedded-tls` for any TLS needs
- Existing JSKV storage for file persistence

## 7. Risk: SFTP Complexity

SFTP requires a full SSH transport layer (key exchange, encryption, MAC). This is significantly more complex than TFTP or FTP. Options:
1. Find a `no_std` SSH crate — may not exist
2. Implement minimal SSH transport — substantial effort
3. Defer SFTP to a later phase — ship TFTP + FTP first

Recommendation: implement TFTP and FTP first, then HTTP/WebSocket extraction. SFTP is Phase 2 — evaluate `no_std` SSH crates before committing to a custom implementation. If no suitable crate exists, scope a minimal SSH transport (diffie-hellman key exchange, AES encryption, HMAC) as a separate design.
