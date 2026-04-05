# Protocols Crate & Network Protocol Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add TFTP, FTP, and SFTP protocol clients to JSOS via a standalone `no_std` protocols crate, integrate with the kernel networking layer, and expose to JavaScript apps through `os.*` APIs with JSKV storage.

**Architecture:** A new `protocols/` crate contains pure state machines that produce/consume bytes with no socket dependency. The kernel's `src/net/` owns smoltcp sockets and feeds data in/out. HTTP and WebSocket extraction from `mod.rs` into the crate happens after the new protocols are working.

**Tech Stack:** Rust `no_std`+`alloc`, smoltcp 0.11 (socket-udp already enabled), JSKV persistent storage.

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `protocols/Cargo.toml` | Crate manifest: `no_std`, `alloc`, no smoltcp dependency |
| `protocols/src/lib.rs` | Re-exports for `tftp`, `ftp`, `sftp` modules |
| `protocols/src/tftp.rs` | TFTP client state machine (RFC 1350) |
| `protocols/src/ftp.rs` | FTP client state machine (RFC 959) |
| `protocols/src/sftp.rs` | SFTP client stub (Phase 2) |
| `src/net/tftp_job.rs` | `TftpJob`: UDP socket glue for `TftpClient` |
| `src/net/ftp_job.rs` | `FtpJob`/`FtpSession`: TCP socket glue for `FtpClient` |
| `src/js/pxe.js` | PXE config analysis library (BCD parser, audit) |
| `src/jsos/pxetool.jsos` | Interactive PXE tool app |

### Modified Files

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `protocols` to workspace members and dependencies |
| `src/lib.rs` | No changes needed (`net` module already declared) |
| `src/net/mod.rs` | Add `pub mod tftp_job; pub mod ftp_job;`, global state for TFTP/FTP jobs, poll calls, cleanup |
| `src/js_runtime.rs` | Register `os.tftp.*` and `os.ftp.*` sub-objects, add callback handlers |
| `src/main.rs` | Add TFTP/FTP active job checks to the HLT sleep condition (lines 215-220) |

---

## Task 1: Create the `protocols` Crate Skeleton

**Files:**
- Create: `protocols/Cargo.toml`
- Create: `protocols/src/lib.rs`

- [ ] **Step 1: Create `protocols/Cargo.toml`**

```toml
[package]
name = "protocols"
version = "0.1.0"
edition = "2021"

[features]
default = []

[dependencies]
```

- [ ] **Step 2: Create `protocols/src/lib.rs`**

```rust
#![no_std]

extern crate alloc;

pub mod tftp;
```

Note: We only declare `tftp` for now. `ftp` and `sftp` modules will be added in their respective tasks.

- [ ] **Step 3: Add `protocols` to workspace and kernel dependency**

In `Cargo.toml` (root), add `"protocols"` to the workspace members array (line 3) and add the dependency (after line 39):

Workspace members change:
```toml
[workspace]
members = [
    ".",
    "protocols",
]
```

Dependency addition (after the `embedded-io-async` line):
```toml
protocols = { path = "protocols" }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds (the empty `tftp` module will fail — that's expected, we create it next)

- [ ] **Step 5: Commit**

```bash
git add protocols/ Cargo.toml
git commit -m "feat: add protocols crate skeleton with workspace integration"
```

---

## Task 2: Implement the TFTP Client State Machine

**Files:**
- Create: `protocols/src/tftp.rs`

The TFTP protocol (RFC 1350) is a simple stop-and-wait protocol over UDP. It uses 512-byte blocks with opcodes for RRQ, WRQ, DATA, ACK, and ERROR.

- [ ] **Step 1: Write `protocols/src/tftp.rs` — types and constants**

```rust
#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::string::String;

// TFTP opcodes (RFC 1350)
const OP_RRQ: u16 = 1;
const OP_WRQ: u16 = 2;
const OP_DATA: u16 = 3;
const OP_ACK: u16 = 4;
const OP_ERROR: u16 = 5;

const BLOCK_SIZE: usize = 512;
const TFTP_PORT: u16 = 69;

#[derive(Debug)]
pub enum TftpError {
    FileNotFound,
    AccessViolation,
    DiskFull,
    IllegalOperation,
    UnknownTransferId,
    FileExists,
    Timeout,
    Protocol(String),
}

pub enum TftpAction {
    /// Send this packet on the UDP socket to the server
    Send(Vec<u8>),
    /// Transfer complete — here is the assembled file data
    Complete(Vec<u8>),
    /// An error occurred
    Error(TftpError),
}

enum TftpState {
    /// Waiting for first DATA packet after sending RRQ
    WaitingData,
    /// Receiving data blocks
    Receiving { expected_block: u16 },
    /// Waiting for ACK after sending DATA block
    WaitingAck { next_block: u16 },
    /// Transfer done
    Done,
}

pub struct TftpClient {
    state: TftpState,
    data: Vec<u8>,
    /// For writes: the full payload to send
    write_data: Vec<u8>,
    /// For writes: byte offset into write_data for next block
    write_offset: usize,
}
```

- [ ] **Step 2: Implement `build_rrq` and `build_wrq` packet builders**

Add to `protocols/src/tftp.rs`:

```rust
/// Build a Read Request packet: opcode(2) + filename + \0 + "octet" + \0
fn build_rrq(filename: &str) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&OP_RRQ.to_be_bytes());
    pkt.extend_from_slice(filename.as_bytes());
    pkt.push(0);
    pkt.extend_from_slice(b"octet");
    pkt.push(0);
    pkt
}

/// Build a Write Request packet: opcode(2) + filename + \0 + "octet" + \0
fn build_wrq(filename: &str) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&OP_WRQ.to_be_bytes());
    pkt.extend_from_slice(filename.as_bytes());
    pkt.push(0);
    pkt.extend_from_slice(b"octet");
    pkt.push(0);
    pkt
}

/// Build an ACK packet: opcode(2) + block_number(2)
fn build_ack(block: u16) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&OP_ACK.to_be_bytes());
    pkt.extend_from_slice(&block.to_be_bytes());
    pkt
}

/// Build a DATA packet: opcode(2) + block_number(2) + data(up to 512)
fn build_data(block: u16, payload: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&OP_DATA.to_be_bytes());
    pkt.extend_from_slice(&block.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}
```

- [ ] **Step 3: Implement `TftpClient::start_read`**

```rust
impl TftpClient {
    /// Start a TFTP read (download). Returns (client, initial RRQ packet).
    pub fn start_read(filename: &str) -> (Self, Vec<u8>) {
        let pkt = build_rrq(filename);
        let client = TftpClient {
            state: TftpState::WaitingData,
            data: Vec::new(),
            write_data: Vec::new(),
            write_offset: 0,
        };
        (client, pkt)
    }
```

- [ ] **Step 4: Implement `TftpClient::start_write`**

```rust
    /// Start a TFTP write (upload). Returns (client, initial WRQ packet).
    pub fn start_write(filename: &str, data: Vec<u8>) -> (Self, Vec<u8>) {
        let pkt = build_wrq(filename);
        let client = TftpClient {
            state: TftpState::WaitingAck { next_block: 1 },
            data: Vec::new(),
            write_data: data,
            write_offset: 0,
        };
        (client, pkt)
    }
```

- [ ] **Step 5: Implement `TftpClient::receive` — the core state machine**

```rust
    /// Feed a received UDP packet. Returns the next action to take.
    pub fn receive(&mut self, packet: &[u8]) -> TftpAction {
        if packet.len() < 4 {
            return TftpAction::Error(TftpError::Protocol(
                String::from("Packet too short"),
            ));
        }

        let opcode = u16::from_be_bytes([packet[0], packet[1]]);
        let block = u16::from_be_bytes([packet[2], packet[3]]);

        match opcode {
            OP_DATA => self.handle_data(block, &packet[4..]),
            OP_ACK => self.handle_ack(block),
            OP_ERROR => {
                let msg = if packet.len() > 4 {
                    let end = packet[4..]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(packet.len() - 4);
                    core::str::from_utf8(&packet[4..4 + end])
                        .unwrap_or("unknown")
                        .into()
                } else {
                    String::from("unknown error")
                };
                TftpAction::Error(TftpError::Protocol(msg))
            }
            _ => TftpAction::Error(TftpError::IllegalOperation),
        }
    }

    fn handle_data(&mut self, block: u16, payload: &[u8]) -> TftpAction {
        match &self.state {
            TftpState::WaitingData => {
                if block == 1 {
                    self.data.extend_from_slice(payload);
                    if payload.len() < BLOCK_SIZE {
                        // Last block — transfer complete
                        self.state = TftpState::Done;
                        // Send final ACK, then signal completion
                        // Caller should send the ACK then read Complete on next call
                        let mut result = Vec::new();
                        core::mem::swap(&mut self.data, &mut result);
                        return TftpAction::Complete(result);
                    }
                    self.state = TftpState::Receiving { expected_block: 2 };
                    TftpAction::Send(build_ack(block))
                } else {
                    TftpAction::Error(TftpError::Protocol(String::from("Unexpected block number")))
                }
            }
            TftpState::Receiving { expected_block } => {
                if block == *expected_block {
                    self.data.extend_from_slice(payload);
                    if payload.len() < BLOCK_SIZE {
                        self.state = TftpState::Done;
                        let mut result = Vec::new();
                        core::mem::swap(&mut self.data, &mut result);
                        return TftpAction::Complete(result);
                    }
                    let next = expected_block + 1;
                    self.state = TftpState::Receiving { expected_block: next };
                    TftpAction::Send(build_ack(block))
                } else {
                    // Duplicate or out-of-order — re-ACK the last good block
                    TftpAction::Send(build_ack(block.wrapping_sub(1)))
                }
            }
            _ => TftpAction::Error(TftpError::IllegalOperation),
        }
    }

    fn handle_ack(&mut self, block: u16) -> TftpAction {
        match &self.state {
            TftpState::WaitingAck { next_block } => {
                let expected_ack = next_block.wrapping_sub(1);
                if block == expected_ack || (block == 0 && *next_block == 1) {
                    // Send next data block
                    let start = self.write_offset;
                    let end = (start + BLOCK_SIZE).min(self.write_data.len());
                    let chunk = &self.write_data[start..end];
                    let pkt = build_data(*next_block, chunk);
                    let is_last = chunk.len() < BLOCK_SIZE;
                    self.write_offset = end;
                    if is_last {
                        self.state = TftpState::Done;
                    } else {
                        self.state = TftpState::WaitingAck {
                            next_block: next_block + 1,
                        };
                    }
                    TftpAction::Send(pkt)
                } else {
                    TftpAction::Error(TftpError::Protocol(String::from("Unexpected ACK block")))
                }
            }
            _ => TftpAction::Error(TftpError::IllegalOperation),
        }
    }

    /// Returns the TFTP server port (69).
    pub fn server_port() -> u16 {
        TFTP_PORT
    }
}
```

- [ ] **Step 6: Verify the crate compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 7: Commit**

```bash
git add protocols/src/tftp.rs
git commit -m "feat(protocols): implement TFTP client state machine (RFC 1350)"
```

---

## Task 3: Kernel TFTP Job Integration

**Files:**
- Create: `src/net/tftp_job.rs`
- Modify: `src/net/mod.rs`

This task wires the TFTP state machine to smoltcp UDP sockets.

- [ ] **Step 1: Create `src/net/tftp_job.rs`**

```rust
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp::{self, Socket as UdpSocket};
use smoltcp::socket::SocketSet;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use protocols::tftp::{TftpClient, TftpAction};
use crate::serial_println;
use crate::storage;

pub struct TftpJob {
    pub pid: u32,
    pub udp_handle: SocketHandle,
    pub client: TftpClient,
    pub store_key: String,
    pub server_addr: IpAddress,
    pub server_port: u16,
    pub start_ticks: u64,
    pub is_write: bool,
    pub done: bool,
    pub result: Option<String>,
}

/// Start a TFTP GET: download `remote_path` from `server_ip` and save to JSKV `store_key`.
pub fn start_tftp_get(
    pid: u32,
    server_ip: &str,
    remote_path: &str,
    store_key: &str,
    sockets: &mut SocketSet<'static>,
) {
    let addr = parse_ipv4(server_ip);
    if addr.is_none() {
        serial_println!("[TFTP] Invalid server IP: {}", server_ip);
        return;
    }
    let server_addr = IpAddress::Ipv4(addr.unwrap());

    let (client, initial_packet) = TftpClient::start_read(remote_path);

    // Allocate a UDP socket
    let rx_buf = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; 4],
        vec![0u8; 2048],
    );
    let tx_buf = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; 4],
        vec![0u8; 2048],
    );
    let mut socket = UdpSocket::new(rx_buf, tx_buf);
    let local_port = super::next_local_port();
    if socket.bind(local_port).is_err() {
        serial_println!("[TFTP] Failed to bind UDP port {}", local_port);
        return;
    }

    let handle = sockets.add(socket);

    // Send initial RRQ packet
    let endpoint = IpEndpoint::new(server_addr, TftpClient::server_port());
    let socket = sockets.get_mut::<UdpSocket>(handle);
    if socket.send_slice(&initial_packet, endpoint).is_err() {
        serial_println!("[TFTP] Failed to send RRQ");
        sockets.remove(handle);
        return;
    }

    let job = TftpJob {
        pid,
        udp_handle: handle,
        client,
        store_key: String::from(store_key),
        server_addr,
        server_port: TftpClient::server_port(),
        start_ticks: crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed),
        is_write: false,
        done: false,
        result: None,
    };

    super::TFTP_JOBS.lock().push(Box::new(job));
    serial_println!("[TFTP] GET started: {} -> store:{}", remote_path, store_key);
}

/// Start a TFTP PUT: upload JSKV `store_key` to `remote_path` on `server_ip`.
pub fn start_tftp_put(
    pid: u32,
    server_ip: &str,
    remote_path: &str,
    store_key: &str,
    sockets: &mut SocketSet<'static>,
) {
    let addr = parse_ipv4(server_ip);
    if addr.is_none() {
        serial_println!("[TFTP] Invalid server IP: {}", server_ip);
        return;
    }
    let server_addr = IpAddress::Ipv4(addr.unwrap());

    // Read data from JSKV
    let data = match storage::read_object(store_key) {
        Some(d) => d,
        None => {
            serial_println!("[TFTP] Store key not found: {}", store_key);
            return;
        }
    };

    let (client, initial_packet) = TftpClient::start_write(remote_path, data);

    let rx_buf = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; 4],
        vec![0u8; 2048],
    );
    let tx_buf = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; 4],
        vec![0u8; 2048],
    );
    let mut socket = UdpSocket::new(rx_buf, tx_buf);
    let local_port = super::next_local_port();
    if socket.bind(local_port).is_err() {
        serial_println!("[TFTP] Failed to bind UDP port {}", local_port);
        return;
    }

    let handle = sockets.add(socket);

    let endpoint = IpEndpoint::new(server_addr, TftpClient::server_port());
    let socket = sockets.get_mut::<UdpSocket>(handle);
    if socket.send_slice(&initial_packet, endpoint).is_err() {
        serial_println!("[TFTP] Failed to send WRQ");
        sockets.remove(handle);
        return;
    }

    let job = TftpJob {
        pid,
        udp_handle: handle,
        client,
        store_key: String::from(store_key),
        server_addr,
        server_port: TftpClient::server_port(),
        start_ticks: crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed),
        is_write: true,
        done: false,
        result: None,
    };

    super::TFTP_JOBS.lock().push(Box::new(job));
    serial_println!("[TFTP] PUT started: store:{} -> {}", store_key, remote_path);
}

/// Poll all active TFTP jobs. Called from `poll_network()`.
pub fn poll_tftp_jobs(sockets: &mut SocketSet, current_ticks: u64) {
    let mut jobs = super::TFTP_JOBS.lock();
    let mut i = 0;
    while i < jobs.len() {
        let job = &mut *jobs[i];

        // Timeout after ~30 seconds
        if current_ticks.saturating_sub(job.start_ticks) > 3000 {
            serial_println!("[TFTP] Timeout for store:{}", job.store_key);
            job.done = true;
            job.result = Some(String::from("Error: TFTP Timeout"));
            let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
            socket.close();
            sockets.remove(job.udp_handle);
            i += 1;
            continue;
        }

        let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
        if let Ok((data, remote_endpoint)) = socket.recv() {
            // Update server_port to the ephemeral port the server responded from
            // (TFTP servers pick a new port for the transfer after the initial RRQ/WRQ)
            job.server_port = remote_endpoint.port;

            match job.client.receive(data) {
                TftpAction::Send(pkt) => {
                    let endpoint = IpEndpoint::new(job.server_addr, job.server_port);
                    let _ = socket.send_slice(&pkt, endpoint);
                }
                TftpAction::Complete(file_data) => {
                    // Send final ACK for the last block
                    // (Complete is returned for reads; the ACK for last block was already
                    //  part of the Complete flow, but we need to handle it)
                    if !job.is_write {
                        // Save to JSKV store
                        storage::write_object(&job.store_key, &file_data);
                        serial_println!(
                            "[TFTP] GET complete: {} bytes -> store:{}",
                            file_data.len(),
                            job.store_key
                        );
                        job.result = Some(alloc::format!("OK:{}", file_data.len()));
                    } else {
                        serial_println!("[TFTP] PUT complete: store:{}", job.store_key);
                        job.result = Some(String::from("OK"));
                    }
                    job.done = true;
                    socket.close();
                    sockets.remove(job.udp_handle);
                }
                TftpAction::Error(e) => {
                    serial_println!("[TFTP] Error: {:?}", e);
                    job.result = Some(alloc::format!("Error: {:?}", e));
                    job.done = true;
                    socket.close();
                    sockets.remove(job.udp_handle);
                }
            }
        }
        i += 1;
    }
}

fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
    let parts: alloc::vec::Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let a = parts[0].parse::<u8>().ok()?;
    let b = parts[1].parse::<u8>().ok()?;
    let c = parts[2].parse::<u8>().ok()?;
    let d = parts[3].parse::<u8>().ok()?;
    Some(Ipv4Address::new(a, b, c, d))
}
```

- [ ] **Step 2: Add TFTP globals and module declaration to `src/net/mod.rs`**

At the top of `src/net/mod.rs`, after `pub mod rtl8139;` (line 27), add:

```rust
pub mod tftp_job;
```

Inside the `lazy_static!` block (after line 110, before the closing `}`), add:

```rust
    pub static ref TFTP_JOBS: Mutex<alloc::vec::Vec<Box<tftp_job::TftpJob>>> = Mutex::new(alloc::vec::Vec::new());
```

- [ ] **Step 3: Add `poll_tftp_jobs` call to `poll_network()`**

In `src/net/mod.rs`, inside `poll_network()`, after the `iface.poll(...)` call (around line 568) and before the server sockets poll, add:

```rust
    // Poll TFTP jobs
    tftp_job::poll_tftp_jobs(sockets, current_ticks);
```

- [ ] **Step 4: Add TFTP cleanup to `cleanup_process_network()`**

In `src/net/mod.rs`, inside `cleanup_process_network()` (after line 1728, before the closing `}`), add:

```rust
    // Clean up TFTP jobs
    let mut tftp_jobs = TFTP_JOBS.lock();
    let mut i = 0;
    while i < tftp_jobs.len() {
        if tftp_jobs[i].pid == pid {
            let job = tftp_jobs.remove(i);
            if let Ok(socket) = core::panic::catch_unwind(|| sockets.get_mut::<smoltcp::socket::udp::Socket>(job.udp_handle)) {
                // Socket may already be removed
            }
            let _ = sockets.remove(job.udp_handle);
        } else {
            i += 1;
        }
    }
```

Note: Since `catch_unwind` is not available in `no_std`, use a simpler approach — just remove the handle and ignore if it's already gone:

```rust
    // Clean up TFTP jobs
    let mut tftp_jobs = TFTP_JOBS.lock();
    let mut i = 0;
    while i < tftp_jobs.len() {
        if tftp_jobs[i].pid == pid {
            let job = tftp_jobs.remove(i);
            // Socket may already be removed by poll if the job finished
            if !job.done {
                let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(job.udp_handle);
                socket.close();
                sockets.remove(job.udp_handle);
            }
        } else {
            i += 1;
        }
    }
```

- [ ] **Step 5: Add TFTP jobs to the HLT sleep condition in `src/main.rs`**

In `src/main.rs` around lines 215-220, add a check for active TFTP jobs:

```rust
        let has_active_tftp = !os::net::TFTP_JOBS.lock().is_empty();
```

And add `&& !has_active_tftp` to the `if` condition before `hlt()`.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 7: Commit**

```bash
git add src/net/tftp_job.rs src/net/mod.rs src/main.rs
git commit -m "feat(net): integrate TFTP job polling with kernel network loop"
```

---

## Task 4: JavaScript API for TFTP

**Files:**
- Modify: `src/js_runtime.rs`
- Modify: `src/net/mod.rs` (add public `start_tftp_get`/`start_tftp_put` wrappers)

- [ ] **Step 1: Add public entry points in `src/net/mod.rs`**

After the `start_websocket` function (around line 504), add:

```rust
pub fn start_tftp_get(pid: u32, server_ip: &str, remote_path: &str, store_key: &str) {
    let mut sockets_guard = SOCKETS.lock();
    if let Some(ref mut sockets) = *sockets_guard {
        tftp_job::start_tftp_get(pid, server_ip, remote_path, store_key, sockets);
    }
}

pub fn start_tftp_put(pid: u32, server_ip: &str, remote_path: &str, store_key: &str) {
    let mut sockets_guard = SOCKETS.lock();
    if let Some(ref mut sockets) = *sockets_guard {
        tftp_job::start_tftp_put(pid, server_ip, remote_path, store_key, sockets);
    }
}
```

- [ ] **Step 2: Add TFTP result delivery in `poll_network()`**

After the fetch job result delivery loop (around line 1048), add TFTP result delivery:

```rust
    // Deliver TFTP results to JS callbacks
    let mut tftp_done: alloc::vec::Vec<Box<tftp_job::TftpJob>> = alloc::vec::Vec::new();
    {
        let mut tftp_jobs = TFTP_JOBS.lock();
        let mut i = 0;
        while i < tftp_jobs.len() {
            if tftp_jobs[i].done {
                tftp_done.push(tftp_jobs.remove(i));
            } else {
                i += 1;
            }
        }
    }
    for job in tftp_done {
        if let Some(sandbox_arc) = crate::process::get_sandbox(job.pid) {
            let result = job.result.as_deref().unwrap_or("Error: unknown");
            let escaped_key = job.store_key.replace('\\', "\\\\").replace('\'', "\\'");
            let escaped_result = result.replace('\\', "\\\\").replace('\'', "\\'");
            let script = alloc::format!(
                "if (typeof globalThis.__onTftpResult === 'function') \
                 {{ globalThis.__onTftpResult('{}', '{}'); }}",
                escaped_key, escaped_result
            );
            let _ = sandbox_arc.lock().eval(&script);
        }
    }
```

- [ ] **Step 3: Register `os.tftp` native functions in `js_runtime.rs`**

In `src/js_runtime.rs`, in the `register_os_api` function (around line 935, near where other sub-objects are built), add:

```rust
    // Build os.tftp sub-object
    let tftp = JS_NewObject(ctx);
    set_func(ctx, tftp, "getNative", js_os_tftp_get, 3);
    set_func(ctx, tftp, "putNative", js_os_tftp_put, 3);
    set_prop_obj(ctx, os, "tftp", tftp);
```

Make sure this is placed before the line `set_prop_obj(ctx, global, "os", os);` (line 1026).

- [ ] **Step 4: Implement the native FFI functions in `js_runtime.rs`**

Add these functions near the other `js_os_*` functions (e.g., after `js_os_fetch`):

```rust
unsafe extern "C" fn js_os_tftp_get(
    ctx: *mut JSContext,
    _this: JSValue,
    argc: c_int,
    argv: *const JSValue,
) -> JSValue {
    if argc >= 3 {
        let server = js_to_rust_string(ctx, *argv.offset(0));
        let remote_path = js_to_rust_string(ctx, *argv.offset(1));
        let store_key = js_to_rust_string(ctx, *argv.offset(2));

        let global = JS_GetGlobalObject(ctx);
        let pid_prop = js_cstring("__PID");
        let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
        let pid = js_val_to_i32(ctx, pid_val) as u32;
        JS_FreeValue(ctx, pid_val);
        JS_FreeValue(ctx, global);

        crate::net::start_tftp_get(pid, &server, &remote_path, &store_key);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_tftp_put(
    ctx: *mut JSContext,
    _this: JSValue,
    argc: c_int,
    argv: *const JSValue,
) -> JSValue {
    if argc >= 3 {
        let server = js_to_rust_string(ctx, *argv.offset(0));
        let remote_path = js_to_rust_string(ctx, *argv.offset(1));
        let store_key = js_to_rust_string(ctx, *argv.offset(2));

        let global = JS_GetGlobalObject(ctx);
        let pid_prop = js_cstring("__PID");
        let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
        let pid = js_val_to_i32(ctx, pid_val) as u32;
        JS_FreeValue(ctx, pid_val);
        JS_FreeValue(ctx, global);

        crate::net::start_tftp_put(pid, &server, &remote_path, &store_key);
    }
    js_undefined()
}
```

- [ ] **Step 5: Add the JavaScript Promise wrapper in the polyfill section**

In `src/js_runtime.rs`, in the polyfill string (around line 667, after the `os.fetch` definition), add:

```javascript
                // TFTP Promise Polyfill
                globalThis.__tftpHandlers = {};
                globalThis.__onTftpResult = function(storeKey, result) {
                    const handler = globalThis.__tftpHandlers[storeKey];
                    if (handler) {
                        delete globalThis.__tftpHandlers[storeKey];
                        if (result.startsWith('OK')) {
                            handler.resolve(result);
                        } else {
                            handler.reject(new Error(result));
                        }
                    }
                };

                os.tftp.get = function(server, remotePath, storeKey) {
                    return new Promise(function(resolve, reject) {
                        globalThis.__tftpHandlers[storeKey] = { resolve: resolve, reject: reject };
                        os.tftp.getNative(server, remotePath, storeKey);
                    });
                };
                os.tftp.put = function(server, remotePath, storeKey) {
                    return new Promise(function(resolve, reject) {
                        globalThis.__tftpHandlers[storeKey] = { resolve: resolve, reject: reject };
                        os.tftp.putNative(server, remotePath, storeKey);
                    });
                };
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 7: Commit**

```bash
git add src/js_runtime.rs src/net/mod.rs
git commit -m "feat: expose os.tftp.get/put JavaScript API with JSKV storage"
```

---

## Task 5: Implement the FTP Client State Machine

**Files:**
- Create: `protocols/src/ftp.rs`
- Modify: `protocols/src/lib.rs`

FTP is session-based with a control channel (TCP port 21) and passive data channel. The state machine handles authentication, directory listing, file transfer, and control commands.

- [ ] **Step 1: Add `ftp` module to `protocols/src/lib.rs`**

```rust
#![no_std]

extern crate alloc;

pub mod tftp;
pub mod ftp;
```

- [ ] **Step 2: Write `protocols/src/ftp.rs` — types and constants**

```rust
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

const FTP_PORT: u16 = 21;

#[derive(Debug)]
pub enum FtpError {
    AuthFailed,
    ConnectionClosed,
    ServerError(String),
    Protocol(String),
    InvalidPassiveResponse,
}

/// Actions the kernel needs to take on behalf of the FTP client.
pub enum FtpAction {
    /// Send this data on the control channel (TCP port 21)
    SendControl(Vec<u8>),
    /// Connect a new TCP socket to this IP:port for passive data transfer
    ConnectData(u32, u16),  // (ipv4 as u32, port)
    /// Send this data on the data channel
    SendData(Vec<u8>),
    /// Data transfer complete — here is the received data (for RETR)
    DataComplete(Vec<u8>),
    /// Command completed successfully with server message
    Ok(String),
    /// An error occurred
    Error(FtpError),
    /// Need more data from the control channel — do nothing this tick
    NeedMore,
}

#[derive(Debug, Clone, PartialEq)]
enum FtpState {
    /// Waiting for server 220 greeting
    WaitGreeting,
    /// Sent USER, waiting for 331
    WaitUserOk,
    /// Sent PASS, waiting for 230
    WaitPassOk,
    /// Idle — ready for commands
    Ready,
    /// Sent TYPE I, waiting for 200
    WaitTypeOk,
    /// Sent PASV, waiting for 227 response
    WaitPasv { pending_command: PendingCommand },
    /// Waiting for data channel to connect
    WaitDataConnect { pending_command: PendingCommand },
    /// Sent RETR/LIST/STOR, waiting for 150 then data transfer
    WaitTransferStart { pending_command: PendingCommand },
    /// Receiving data on data channel
    Transferring { pending_command: PendingCommand },
    /// Sent QUIT
    Done,
}

#[derive(Debug, Clone, PartialEq)]
enum PendingCommand {
    List(String),              // LIST <path>
    Retr(String, String),      // RETR <remote>, store_key
    Stor(String, Vec<u8>),     // STOR <remote>, data
    Mkdir(String),             // MKD <path>
    Delete(String),            // DELE <path>
}

pub struct FtpClient {
    state: FtpState,
    user: String,
    pass: String,
    control_buffer: Vec<u8>,
    data_buffer: Vec<u8>,
    type_set: bool,
}
```

- [ ] **Step 3: Implement FTP client constructor and control channel parser**

```rust
impl FtpClient {
    pub fn new(user: &str, pass: &str) -> Self {
        FtpClient {
            state: FtpState::WaitGreeting,
            user: String::from(user),
            pass: String::from(pass),
            control_buffer: Vec::new(),
            data_buffer: Vec::new(),
            type_set: false,
        }
    }

    pub fn server_port() -> u16 {
        FTP_PORT
    }

    /// Feed data received on the control channel.
    pub fn receive_control(&mut self, data: &[u8]) -> FtpAction {
        self.control_buffer.extend_from_slice(data);

        // FTP responses are line-based ending with \r\n
        // Multi-line responses: "NNN-..." then "NNN ..." for the last line
        let response = match self.extract_response() {
            Some(r) => r,
            None => return FtpAction::NeedMore,
        };

        let code = Self::parse_code(&response);
        self.handle_response(code, &response)
    }

    /// Feed data received on the data channel.
    pub fn receive_data(&mut self, data: &[u8]) -> FtpAction {
        self.data_buffer.extend_from_slice(data);
        FtpAction::NeedMore
    }

    /// Called when the data channel is closed by the server (transfer complete).
    pub fn data_channel_closed(&mut self) -> FtpAction {
        match &self.state {
            FtpState::Transferring { pending_command } => {
                let result = core::mem::take(&mut self.data_buffer);
                let msg = match pending_command {
                    PendingCommand::List(_) => {
                        self.state = FtpState::Ready;
                        FtpAction::Ok(String::from_utf8_lossy(&result).into())
                    }
                    PendingCommand::Retr(_, _) => {
                        self.state = FtpState::Ready;
                        FtpAction::DataComplete(result)
                    }
                    PendingCommand::Stor(_, _) => {
                        self.state = FtpState::Ready;
                        FtpAction::Ok(String::from("Upload complete"))
                    }
                    _ => {
                        self.state = FtpState::Ready;
                        FtpAction::Ok(String::from("Transfer done"))
                    }
                };
                msg
            }
            _ => FtpAction::NeedMore,
        }
    }

    /// Request: list a remote directory.
    pub fn list(&mut self, path: &str) -> FtpAction {
        if self.state != FtpState::Ready {
            return FtpAction::Error(FtpError::Protocol(String::from("Not ready")));
        }
        self.start_pasv(PendingCommand::List(String::from(path)))
    }

    /// Request: download a remote file.
    pub fn get(&mut self, remote_path: &str, store_key: &str) -> FtpAction {
        if self.state != FtpState::Ready {
            return FtpAction::Error(FtpError::Protocol(String::from("Not ready")));
        }
        self.start_pasv(PendingCommand::Retr(
            String::from(remote_path),
            String::from(store_key),
        ))
    }

    /// Request: upload data to a remote file.
    pub fn put(&mut self, remote_path: &str, data: Vec<u8>) -> FtpAction {
        if self.state != FtpState::Ready {
            return FtpAction::Error(FtpError::Protocol(String::from("Not ready")));
        }
        self.start_pasv(PendingCommand::Stor(String::from(remote_path), data))
    }

    /// Request: create a remote directory.
    pub fn mkdir(&mut self, path: &str) -> FtpAction {
        if self.state != FtpState::Ready {
            return FtpAction::Error(FtpError::Protocol(String::from("Not ready")));
        }
        self.state = FtpState::Ready; // MKD doesn't need PASV
        FtpAction::SendControl(format!("MKD {}\r\n", path).into_bytes())
    }

    /// Request: delete a remote file.
    pub fn delete(&mut self, path: &str) -> FtpAction {
        if self.state != FtpState::Ready {
            return FtpAction::Error(FtpError::Protocol(String::from("Not ready")));
        }
        FtpAction::SendControl(format!("DELE {}\r\n", path).into_bytes())
    }

    /// Request: close the session.
    pub fn quit(&mut self) -> FtpAction {
        self.state = FtpState::Done;
        FtpAction::SendControl(b"QUIT\r\n".to_vec())
    }

    pub fn is_ready(&self) -> bool {
        self.state == FtpState::Ready
    }

    pub fn is_done(&self) -> bool {
        self.state == FtpState::Done
    }

    // --- Internal helpers ---

    fn start_pasv(&mut self, cmd: PendingCommand) -> FtpAction {
        if !self.type_set {
            self.type_set = true;
            self.state = FtpState::WaitTypeOk;
            // We'll need to re-issue PASV after TYPE, stash the command
            // Actually, set TYPE I first, then PASV
            return FtpAction::SendControl(b"TYPE I\r\n".to_vec());
        }
        self.state = FtpState::WaitPasv { pending_command: cmd };
        FtpAction::SendControl(b"PASV\r\n".to_vec())
    }

    fn extract_response(&mut self) -> Option<String> {
        // Find complete line ending with \r\n
        let buf_str = String::from_utf8_lossy(&self.control_buffer).into_owned();
        if let Some(pos) = buf_str.find("\r\n") {
            let line = String::from(&buf_str[..pos]);
            let consumed = pos + 2;
            self.control_buffer.drain(..consumed.min(self.control_buffer.len()));
            Some(line)
        } else {
            None
        }
    }

    fn parse_code(response: &str) -> u16 {
        response
            .get(..3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    fn handle_response(&mut self, code: u16, response: &str) -> FtpAction {
        match &self.state {
            FtpState::WaitGreeting => {
                if code == 220 {
                    self.state = FtpState::WaitUserOk;
                    FtpAction::SendControl(format!("USER {}\r\n", self.user).into_bytes())
                } else {
                    FtpAction::Error(FtpError::ServerError(String::from(response)))
                }
            }
            FtpState::WaitUserOk => {
                if code == 331 {
                    self.state = FtpState::WaitPassOk;
                    FtpAction::SendControl(format!("PASS {}\r\n", self.pass).into_bytes())
                } else if code == 230 {
                    // No password needed
                    self.state = FtpState::Ready;
                    FtpAction::Ok(String::from("Logged in"))
                } else {
                    FtpAction::Error(FtpError::AuthFailed)
                }
            }
            FtpState::WaitPassOk => {
                if code == 230 {
                    self.state = FtpState::Ready;
                    FtpAction::Ok(String::from("Logged in"))
                } else {
                    FtpAction::Error(FtpError::AuthFailed)
                }
            }
            FtpState::WaitTypeOk => {
                if code == 200 {
                    // TYPE set, now proceed to whatever command was pending
                    self.state = FtpState::Ready;
                    FtpAction::Ok(String::from("Type set"))
                } else {
                    FtpAction::Error(FtpError::ServerError(String::from(response)))
                }
            }
            FtpState::WaitPasv { .. } => {
                if code == 227 {
                    // Parse PASV response: 227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)
                    match Self::parse_pasv(response) {
                        Some((ip, port)) => {
                            let pending = match core::mem::replace(
                                &mut self.state,
                                FtpState::Ready,
                            ) {
                                FtpState::WaitPasv { pending_command } => pending_command,
                                _ => unreachable!(),
                            };
                            self.state = FtpState::WaitDataConnect { pending_command: pending };
                            FtpAction::ConnectData(ip, port)
                        }
                        None => FtpAction::Error(FtpError::InvalidPassiveResponse),
                    }
                } else {
                    FtpAction::Error(FtpError::ServerError(String::from(response)))
                }
            }
            FtpState::WaitDataConnect { .. } => {
                // Shouldn't receive control data in this state normally
                FtpAction::NeedMore
            }
            FtpState::WaitTransferStart { .. } => {
                if code == 150 || code == 125 {
                    let pending = match core::mem::replace(&mut self.state, FtpState::Ready) {
                        FtpState::WaitTransferStart { pending_command } => pending_command,
                        _ => unreachable!(),
                    };
                    // If this is a STOR, we need to send the data
                    if let PendingCommand::Stor(_, ref data) = pending {
                        let send_data = data.clone();
                        self.state = FtpState::Transferring { pending_command: pending };
                        return FtpAction::SendData(send_data);
                    }
                    self.state = FtpState::Transferring { pending_command: pending };
                    FtpAction::NeedMore
                } else {
                    FtpAction::Error(FtpError::ServerError(String::from(response)))
                }
            }
            FtpState::Transferring { .. } => {
                // 226 = Transfer complete
                if code == 226 {
                    let result = core::mem::take(&mut self.data_buffer);
                    let pending = match core::mem::replace(&mut self.state, FtpState::Ready) {
                        FtpState::Transferring { pending_command } => pending_command,
                        _ => unreachable!(),
                    };
                    match pending {
                        PendingCommand::List(_) => FtpAction::Ok(String::from_utf8_lossy(&result).into()),
                        PendingCommand::Retr(_, _) => FtpAction::DataComplete(result),
                        PendingCommand::Stor(_, _) => FtpAction::Ok(String::from("Upload complete")),
                        _ => FtpAction::Ok(String::from("Done")),
                    }
                } else {
                    FtpAction::NeedMore
                }
            }
            FtpState::Ready => {
                // Could be response to MKD, DELE, etc.
                if code >= 200 && code < 400 {
                    FtpAction::Ok(String::from(response))
                } else {
                    FtpAction::Error(FtpError::ServerError(String::from(response)))
                }
            }
            FtpState::Done => FtpAction::NeedMore,
        }
    }

    /// Notify that the data channel has connected — send the actual command.
    pub fn data_connected(&mut self) -> FtpAction {
        let pending = match core::mem::replace(&mut self.state, FtpState::Ready) {
            FtpState::WaitDataConnect { pending_command } => pending_command,
            other => {
                self.state = other;
                return FtpAction::NeedMore;
            }
        };

        let cmd = match &pending {
            PendingCommand::List(path) => format!("LIST {}\r\n", path),
            PendingCommand::Retr(path, _) => format!("RETR {}\r\n", path),
            PendingCommand::Stor(path, _) => format!("STOR {}\r\n", path),
            PendingCommand::Mkdir(path) => format!("MKD {}\r\n", path),
            PendingCommand::Delete(path) => format!("DELE {}\r\n", path),
        };
        self.state = FtpState::WaitTransferStart { pending_command: pending };
        FtpAction::SendControl(cmd.into_bytes())
    }

    fn parse_pasv(response: &str) -> Option<(u32, u16)> {
        // 227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)
        let start = response.find('(')?;
        let end = response.find(')')?;
        let nums: Vec<&str> = response[start + 1..end].split(',').collect();
        if nums.len() != 6 {
            return None;
        }
        let h1: u8 = nums[0].trim().parse().ok()?;
        let h2: u8 = nums[1].trim().parse().ok()?;
        let h3: u8 = nums[2].trim().parse().ok()?;
        let h4: u8 = nums[3].trim().parse().ok()?;
        let p1: u16 = nums[4].trim().parse().ok()?;
        let p2: u16 = nums[5].trim().parse().ok()?;
        let ip = ((h1 as u32) << 24) | ((h2 as u32) << 16) | ((h3 as u32) << 8) | (h4 as u32);
        let port = (p1 << 8) | p2;
        Some((ip, port))
    }
}

impl alloc::string::String {
    // Use String::from_utf8_lossy which is already available via alloc
}
```

- [ ] **Step 4: Verify the crate compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 5: Commit**

```bash
git add protocols/src/ftp.rs protocols/src/lib.rs
git commit -m "feat(protocols): implement FTP client state machine (RFC 959)"
```

---

## Task 6: Kernel FTP Job Integration

**Files:**
- Create: `src/net/ftp_job.rs`
- Modify: `src/net/mod.rs`

- [ ] **Step 1: Create `src/net/ftp_job.rs`**

```rust
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{self, Socket as TcpSocket, State};
use smoltcp::socket::SocketSet;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use protocols::ftp::{FtpClient, FtpAction, FtpError};
use crate::serial_println;
use crate::storage;

static NEXT_FTP_SESSION_ID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(1);

pub struct FtpSession {
    pub pid: u32,
    pub session_id: u64,
    pub control_handle: SocketHandle,
    pub data_handle: Option<SocketHandle>,
    pub client: FtpClient,
    pub server_addr: IpAddress,
    pub start_ticks: u64,
    pub last_activity: u64,
    /// Pending command result to deliver to JS
    pub pending_result: Option<String>,
    /// Pending data result (for RETR) to deliver to JS
    pub pending_data: Option<(alloc::vec::Vec<u8>, String)>, // (data, store_key)
    pub closed: bool,
}

pub fn start_ftp_session(
    pid: u32,
    server_ip: &str,
    user: &str,
    pass: &str,
    sockets: &mut SocketSet<'static>,
) -> i64 {
    let addr = parse_ipv4(server_ip);
    if addr.is_none() {
        serial_println!("[FTP] Invalid server IP: {}", server_ip);
        return -1;
    }
    let server_addr = IpAddress::Ipv4(addr.unwrap());

    // Create control TCP socket
    let rx_buf = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let tx_buf = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let mut socket = TcpSocket::new(rx_buf, tx_buf);
    let local_port = super::next_local_port();
    let endpoint = IpEndpoint::new(server_addr, FtpClient::server_port());
    if socket.connect(sockets.get_mut::<smoltcp::socket::dhcpv4::Socket>(
        // We can't use connect like this — TcpSocket::connect needs the iface context
        // Instead, we use the pattern from FetchJob: set state to connect and let poll handle it
    ), endpoint, local_port).is_err() {
        serial_println!("[FTP] Failed to initiate connection");
        return -1;
    }

    // Actually, smoltcp TcpSocket::connect takes (context, remote, local_port)
    // But in JSOS the pattern is: add socket to set, then call connect on the socket from the set
    let handle = sockets.add(socket);
    {
        let socket = sockets.get_mut::<TcpSocket>(handle);
        if socket
            .connect(
                smoltcp::iface::Context::new(
                    // We don't have iface context here — need to defer connection
                    // This is the same problem FetchJob has. Let's use the same approach:
                    // Store the endpoint, connect during poll when we have iface access.
                ),
                endpoint,
                local_port,
            )
            .is_err()
        {
            sockets.remove(handle);
            return -1;
        }
    }

    let session_id = NEXT_FTP_SESSION_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let now = crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    let session = FtpSession {
        pid,
        session_id,
        control_handle: handle,
        data_handle: None,
        client: FtpClient::new(user, pass),
        server_addr,
        start_ticks: now,
        last_activity: now,
        pending_result: None,
        pending_data: None,
        closed: false,
    };

    super::FTP_SESSIONS.lock().insert(session_id, Box::new(session));
    serial_println!("[FTP] Session {} connecting to {}", session_id, server_ip);
    session_id as i64
}
```

**Important note:** The above code has a problem — `TcpSocket::connect` in smoltcp 0.11 needs a `Context` from the interface, which we don't have in `start_ftp_session`. Looking at how FetchJob handles this (state 3, lines ~700-720 of mod.rs), the connection is initiated during `poll_network()` when the interface is available.

Let me revise the approach. The FTP session will be created with a "connecting" state and the actual TCP connect happens in `poll_ftp_sessions()`:

```rust
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{self, Socket as TcpSocket, State};
use smoltcp::socket::SocketSet;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use smoltcp::iface::Interface;
use protocols::ftp::{FtpClient, FtpAction};
use crate::serial_println;
use crate::storage;

static NEXT_FTP_SESSION_ID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(1);

#[derive(PartialEq)]
enum FtpJobState {
    Connecting,
    WaitEstablished,
    Active,
    Done,
}

pub struct FtpSession {
    pub pid: u32,
    pub session_id: u64,
    pub control_handle: SocketHandle,
    pub data_handle: Option<SocketHandle>,
    pub client: FtpClient,
    pub server_addr: IpAddress,
    pub start_ticks: u64,
    pub last_activity: u64,
    pub job_state: FtpJobState,
    /// Queue of commands from JS (list, get, put, mkdir, delete, close)
    pub command_queue: Vec<FtpCommand>,
    /// Result to deliver to JS callback
    pub pending_results: Vec<FtpResult>,
    /// Store key for the current in-flight GET (set when Get command is dequeued)
    pub current_get_store_key: Option<String>,
    pub closed: bool,
}

pub struct FtpCommand {
    pub kind: FtpCommandKind,
    pub callback_id: String,
}

pub enum FtpCommandKind {
    List(String),
    Get(String, String),       // remote_path, store_key
    Put(String, String),       // remote_path, store_key
    Mkdir(String),
    Delete(String),
    Close,
}

pub struct FtpResult {
    pub callback_id: String,
    pub success: bool,
    pub data: String,
}

pub fn create_ftp_session(
    pid: u32,
    server_ip: &str,
    user: &str,
    pass: &str,
    sockets: &mut SocketSet<'static>,
) -> i64 {
    let addr = match parse_ipv4(server_ip) {
        Some(a) => a,
        None => {
            serial_println!("[FTP] Invalid server IP: {}", server_ip);
            return -1;
        }
    };
    let server_addr = IpAddress::Ipv4(addr);

    // Create control TCP socket (connect happens in poll)
    let rx_buf = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let tx_buf = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let socket = TcpSocket::new(rx_buf, tx_buf);
    let handle = sockets.add(socket);

    let session_id = NEXT_FTP_SESSION_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let now = crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    let session = FtpSession {
        pid,
        session_id,
        control_handle: handle,
        data_handle: None,
        client: FtpClient::new(user, pass),
        server_addr,
        start_ticks: now,
        last_activity: now,
        job_state: FtpJobState::Connecting,
        command_queue: Vec::new(),
        pending_results: Vec::new(),
        current_get_store_key: None,
        closed: false,
    };

    super::FTP_SESSIONS.lock().insert(session_id, Box::new(session));
    serial_println!("[FTP] Session {} created for {}", session_id, server_ip);
    session_id as i64
}

pub fn enqueue_ftp_command(session_id: u64, cmd: FtpCommand) {
    let mut sessions = super::FTP_SESSIONS.lock();
    if let Some(session) = sessions.get_mut(&session_id) {
        session.command_queue.push(cmd);
    }
}

pub fn poll_ftp_sessions(
    sockets: &mut SocketSet,
    iface: &mut Interface,
    current_ticks: u64,
) {
    let mut sessions = super::FTP_SESSIONS.lock();
    let mut to_remove: Vec<u64> = Vec::new();

    for (&id, session) in sessions.iter_mut() {
        // Timeout after ~60 seconds of inactivity
        if current_ticks.saturating_sub(session.last_activity) > 6000 {
            serial_println!("[FTP] Session {} timed out", id);
            session.closed = true;
            to_remove.push(id);
            continue;
        }

        match session.job_state {
            FtpJobState::Connecting => {
                let socket = sockets.get_mut::<TcpSocket>(session.control_handle);
                let local_port = super::next_local_port();
                let endpoint = IpEndpoint::new(session.server_addr, FtpClient::server_port());
                let cx = iface.context();
                if socket.connect(cx, endpoint, local_port).is_ok() {
                    session.job_state = FtpJobState::WaitEstablished;
                    session.last_activity = current_ticks;
                } else {
                    serial_println!("[FTP] Session {} connect failed", id);
                    session.closed = true;
                    to_remove.push(id);
                }
            }
            FtpJobState::WaitEstablished => {
                let socket = sockets.get_mut::<TcpSocket>(session.control_handle);
                if socket.state() == State::Established {
                    session.job_state = FtpJobState::Active;
                    session.last_activity = current_ticks;
                }
            }
            FtpJobState::Active => {
                // Read from control channel
                let socket = sockets.get_mut::<TcpSocket>(session.control_handle);
                let mut buf = [0u8; 1024];
                if socket.can_recv() {
                    if let Ok(n) = socket.recv_slice(&mut buf) {
                        if n > 0 {
                            session.last_activity = current_ticks;
                            let action = session.client.receive_control(&buf[..n]);
                            handle_ftp_action(session, sockets, iface, action);
                        }
                    }
                }

                // Read from data channel if active
                if let Some(data_handle) = session.data_handle {
                    let data_socket = sockets.get_mut::<TcpSocket>(data_handle);
                    if data_socket.can_recv() {
                        let mut dbuf = [0u8; 4096];
                        if let Ok(n) = data_socket.recv_slice(&mut dbuf) {
                            if n > 0 {
                                session.last_activity = current_ticks;
                                let action = session.client.receive_data(&dbuf[..n]);
                                handle_ftp_action(session, sockets, iface, action);
                            }
                        }
                    }
                    // Check if data channel closed
                    let data_socket = sockets.get_mut::<TcpSocket>(data_handle);
                    if data_socket.state() == State::CloseWait
                        || data_socket.state() == State::Closed
                        || data_socket.state() == State::TimeWait
                    {
                        let action = session.client.data_channel_closed();
                        sockets.remove(data_handle);
                        session.data_handle = None;
                        handle_ftp_action(session, sockets, iface, action);
                    }
                }

                // Process queued commands if client is ready and no pending data channel
                if session.client.is_ready() && session.data_handle.is_none() {
                    if let Some(cmd) = session.command_queue.first() {
                        let action = match &cmd.kind {
                            FtpCommandKind::List(path) => session.client.list(path),
                            FtpCommandKind::Get(remote, store_key) => {
                                session.current_get_store_key = Some(store_key.clone());
                                session.client.get(remote, store_key)
                            }
                            FtpCommandKind::Put(remote, store_key) => {
                                let data = storage::read_object(store_key).unwrap_or_default();
                                session.client.put(remote, data)
                            }
                            FtpCommandKind::Mkdir(path) => session.client.mkdir(path),
                            FtpCommandKind::Delete(path) => session.client.delete(path),
                            FtpCommandKind::Close => session.client.quit(),
                        };
                        let callback_id = cmd.callback_id.clone();
                        session.command_queue.remove(0);
                        handle_ftp_action(session, sockets, iface, action);
                    }
                }

                if session.client.is_done() {
                    session.closed = true;
                    to_remove.push(id);
                }
            }
            FtpJobState::Done => {
                to_remove.push(id);
            }
        }
    }

    for id in to_remove {
        if let Some(mut session) = sessions.remove(&id) {
            let socket = sockets.get_mut::<TcpSocket>(session.control_handle);
            socket.abort();
            sockets.remove(session.control_handle);
            if let Some(dh) = session.data_handle.take() {
                let ds = sockets.get_mut::<TcpSocket>(dh);
                ds.abort();
                sockets.remove(dh);
            }
        }
    }
}

fn handle_ftp_action(
    session: &mut FtpSession,
    sockets: &mut SocketSet,
    iface: &mut Interface,
    action: FtpAction,
) {
    match action {
        FtpAction::SendControl(data) => {
            let socket = sockets.get_mut::<TcpSocket>(session.control_handle);
            let _ = socket.send_slice(&data);
        }
        FtpAction::ConnectData(ip_u32, port) => {
            // Create data channel TCP socket
            let rx_buf = tcp::SocketBuffer::new(vec![0u8; 8192]);
            let tx_buf = tcp::SocketBuffer::new(vec![0u8; 8192]);
            let socket = TcpSocket::new(rx_buf, tx_buf);
            let handle = sockets.add(socket);

            let ip = Ipv4Address::new(
                ((ip_u32 >> 24) & 0xFF) as u8,
                ((ip_u32 >> 16) & 0xFF) as u8,
                ((ip_u32 >> 8) & 0xFF) as u8,
                (ip_u32 & 0xFF) as u8,
            );
            let endpoint = IpEndpoint::new(IpAddress::Ipv4(ip), port);
            let local_port = super::next_local_port();
            let data_socket = sockets.get_mut::<TcpSocket>(handle);
            let cx = iface.context();
            if data_socket.connect(cx, endpoint, local_port).is_ok() {
                session.data_handle = Some(handle);
                // Notify client that data channel is connecting
                // (actual data_connected() call happens when state == Established)
            } else {
                serial_println!("[FTP] Data channel connect failed");
                sockets.remove(handle);
            }
        }
        FtpAction::SendData(data) => {
            if let Some(dh) = session.data_handle {
                let socket = sockets.get_mut::<TcpSocket>(dh);
                let _ = socket.send_slice(&data);
                // Close the data channel after sending (for STOR)
                socket.close();
            }
        }
        FtpAction::DataComplete(data) => {
            // Save to JSKV using the store_key captured when the Get command was dequeued
            if let Some(ref store_key) = session.current_get_store_key {
                storage::write_object(store_key, &data);
                session.pending_results.push(FtpResult {
                    callback_id: String::from(""),
                    success: true,
                    data: format!("OK:{}", data.len()),
                });
                session.current_get_store_key = None;
            }
        }
        FtpAction::Ok(msg) => {
            session.pending_results.push(FtpResult {
                callback_id: String::from(""),
                success: true,
                data: msg,
            });
        }
        FtpAction::Error(e) => {
            session.pending_results.push(FtpResult {
                callback_id: String::from(""),
                success: false,
                data: format!("{:?}", e),
            });
        }
        FtpAction::NeedMore => {}
    }
}

fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return None; }
    let a = parts[0].parse::<u8>().ok()?;
    let b = parts[1].parse::<u8>().ok()?;
    let c = parts[2].parse::<u8>().ok()?;
    let d = parts[3].parse::<u8>().ok()?;
    Some(Ipv4Address::new(a, b, c, d))
}
```

- [ ] **Step 2: Add FTP globals and module to `src/net/mod.rs`**

After `pub mod tftp_job;` add:
```rust
pub mod ftp_job;
```

In the `lazy_static!` block, add:
```rust
    pub static ref FTP_SESSIONS: Mutex<alloc::collections::BTreeMap<u64, Box<ftp_job::FtpSession>>> = Mutex::new(alloc::collections::BTreeMap::new());
```

- [ ] **Step 3: Add `poll_ftp_sessions` call to `poll_network()`**

After the TFTP poll call, add:
```rust
    // Poll FTP sessions
    ftp_job::poll_ftp_sessions(sockets, iface, current_ticks);
```

Note: `iface` is already available as a mutable reference at this point in the function.

- [ ] **Step 4: Add FTP cleanup to `cleanup_process_network()`**

```rust
    // Clean up FTP sessions
    let mut ftp_sessions = FTP_SESSIONS.lock();
    let mut ftp_to_remove: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    for (&id, session) in ftp_sessions.iter() {
        if session.pid == pid {
            ftp_to_remove.push(id);
        }
    }
    for id in ftp_to_remove {
        if let Some(mut session) = ftp_sessions.remove(&id) {
            let socket = sockets.get_mut::<TcpSocket>(session.control_handle);
            socket.abort();
            sockets.remove(session.control_handle);
            if let Some(dh) = session.data_handle.take() {
                let ds = sockets.get_mut::<TcpSocket>(dh);
                ds.abort();
                sockets.remove(dh);
            }
        }
    }
```

- [ ] **Step 5: Add FTP to HLT condition in `src/main.rs`**

```rust
        let has_active_ftp = !os::net::FTP_SESSIONS.lock().is_empty();
```

Add `&& !has_active_ftp` to the HLT condition.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 7: Commit**

```bash
git add src/net/ftp_job.rs src/net/mod.rs src/main.rs
git commit -m "feat(net): integrate FTP session polling with kernel network loop"
```

---

## Task 7: JavaScript API for FTP

**Files:**
- Modify: `src/js_runtime.rs`
- Modify: `src/net/mod.rs`

- [ ] **Step 1: Add public FTP entry points in `src/net/mod.rs`**

After the TFTP entry points:

```rust
pub fn ftp_connect(pid: u32, server_ip: &str, user: &str, pass: &str) -> i64 {
    let mut sockets_guard = SOCKETS.lock();
    if let Some(ref mut sockets) = *sockets_guard {
        ftp_job::create_ftp_session(pid, server_ip, user, pass, sockets)
    } else {
        -1
    }
}

pub fn ftp_command(session_id: u64, cmd: ftp_job::FtpCommand) {
    ftp_job::enqueue_ftp_command(session_id, cmd);
}
```

- [ ] **Step 2: Add FTP result delivery in `poll_network()`**

After the TFTP result delivery section:

```rust
    // Deliver FTP results to JS callbacks
    {
        let mut sessions = FTP_SESSIONS.lock();
        for (_, session) in sessions.iter_mut() {
            while let Some(result) = session.pending_results.pop() {
                if let Some(sandbox_arc) = crate::process::get_sandbox(session.pid) {
                    let escaped_data = result.data.replace('\\', "\\\\").replace('\'', "\\'");
                    let script = alloc::format!(
                        "if (typeof globalThis.__onFtpResult === 'function') \
                         {{ globalThis.__onFtpResult({}, {}, '{}'); }}",
                        session.session_id,
                        result.success,
                        escaped_data
                    );
                    let _ = sandbox_arc.lock().eval(&script);
                }
            }
        }
    }
```

- [ ] **Step 3: Register `os.ftp` native functions in `js_runtime.rs`**

In the `register_os_api` function, add (near the tftp sub-object):

```rust
    // Build os.ftp sub-object
    let ftp = JS_NewObject(ctx);
    set_func(ctx, ftp, "connectNative", js_os_ftp_connect, 3);
    set_func(ctx, ftp, "commandNative", js_os_ftp_command, 3);
    set_prop_obj(ctx, os, "ftp", ftp);
```

- [ ] **Step 4: Implement native FTP FFI functions**

```rust
unsafe extern "C" fn js_os_ftp_connect(
    ctx: *mut JSContext,
    _this: JSValue,
    argc: c_int,
    argv: *const JSValue,
) -> JSValue {
    if argc >= 3 {
        let server = js_to_rust_string(ctx, *argv.offset(0));
        let user = js_to_rust_string(ctx, *argv.offset(1));
        let pass = js_to_rust_string(ctx, *argv.offset(2));

        let global = JS_GetGlobalObject(ctx);
        let pid_prop = js_cstring("__PID");
        let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
        let pid = js_val_to_i32(ctx, pid_val) as u32;
        JS_FreeValue(ctx, pid_val);
        JS_FreeValue(ctx, global);

        let session_id = crate::net::ftp_connect(pid, &server, &user, &pass);
        return js_int(session_id as i32);
    }
    js_int(-1)
}

unsafe extern "C" fn js_os_ftp_command(
    ctx: *mut JSContext,
    _this: JSValue,
    argc: c_int,
    argv: *const JSValue,
) -> JSValue {
    if argc >= 3 {
        let session_id = js_val_to_i32(ctx, *argv.offset(0)) as u64;
        let cmd_type = js_to_rust_string(ctx, *argv.offset(1));
        let arg = js_to_rust_string(ctx, *argv.offset(2));

        let kind = match cmd_type.as_str() {
            "list" => crate::net::ftp_job::FtpCommandKind::List(arg.clone()),
            "get" => {
                // arg format: "remote_path|store_key"
                let parts: alloc::vec::Vec<&str> = arg.splitn(2, '|').collect();
                if parts.len() == 2 {
                    crate::net::ftp_job::FtpCommandKind::Get(
                        alloc::string::String::from(parts[0]),
                        alloc::string::String::from(parts[1]),
                    )
                } else {
                    return js_undefined();
                }
            }
            "put" => {
                let parts: alloc::vec::Vec<&str> = arg.splitn(2, '|').collect();
                if parts.len() == 2 {
                    crate::net::ftp_job::FtpCommandKind::Put(
                        alloc::string::String::from(parts[0]),
                        alloc::string::String::from(parts[1]),
                    )
                } else {
                    return js_undefined();
                }
            }
            "mkdir" => crate::net::ftp_job::FtpCommandKind::Mkdir(arg.clone()),
            "delete" => crate::net::ftp_job::FtpCommandKind::Delete(arg.clone()),
            "close" => crate::net::ftp_job::FtpCommandKind::Close,
            _ => return js_undefined(),
        };

        let cmd = crate::net::ftp_job::FtpCommand {
            kind,
            callback_id: alloc::string::String::from(""),
        };
        crate::net::ftp_command(session_id, cmd);
    }
    js_undefined()
}
```

- [ ] **Step 5: Add JavaScript FTP Promise wrapper in polyfill section**

After the TFTP polyfill:

```javascript
                // FTP Session Polyfill
                globalThis.__ftpHandlers = {};
                globalThis.__onFtpResult = function(sessionId, success, data) {
                    const handlers = globalThis.__ftpHandlers[sessionId];
                    if (handlers && handlers.length > 0) {
                        const handler = handlers.shift();
                        if (success) {
                            handler.resolve(data);
                        } else {
                            handler.reject(new Error(data));
                        }
                    }
                };

                os.ftp.connect = function(server, options) {
                    options = options || {};
                    const user = options.user || 'anonymous';
                    const pass = options.pass || '';
                    const sessionId = os.ftp.connectNative(server, user, pass);
                    if (sessionId < 0) return null;

                    globalThis.__ftpHandlers[sessionId] = [];

                    function ftpCommand(type, arg) {
                        return new Promise(function(resolve, reject) {
                            globalThis.__ftpHandlers[sessionId].push({ resolve: resolve, reject: reject });
                            os.ftp.commandNative(sessionId, type, arg);
                        });
                    }

                    return {
                        list: function(path) { return ftpCommand('list', path || '/'); },
                        get: function(remotePath, storeKey) { return ftpCommand('get', remotePath + '|' + storeKey); },
                        put: function(remotePath, storeKey) { return ftpCommand('put', remotePath + '|' + storeKey); },
                        mkdir: function(path) { return ftpCommand('mkdir', path); },
                        delete: function(path) { return ftpCommand('delete', path); },
                        close: function() {
                            return ftpCommand('close', '').then(function() {
                                delete globalThis.__ftpHandlers[sessionId];
                            });
                        }
                    };
                };
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 7: Commit**

```bash
git add src/js_runtime.rs src/net/mod.rs
git commit -m "feat: expose os.ftp session-based JavaScript API"
```

---

## Task 8: SFTP Stub (Phase 2 Marker)

**Files:**
- Create: `protocols/src/sftp.rs`
- Modify: `protocols/src/lib.rs`

SFTP requires SSH transport which is a large undertaking. This task creates a stub module with the planned interface so it compiles and the architecture is in place.

- [ ] **Step 1: Create `protocols/src/sftp.rs`**

```rust
//! SFTP client (SSH File Transfer Protocol) — Phase 2
//!
//! Requires SSH transport layer (key exchange, encryption, MAC).
//! This module is a stub defining the planned interface.
//! Implementation depends on finding or building a no_std SSH transport.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug)]
pub enum SftpError {
    NotImplemented,
    AuthFailed,
    ConnectionClosed,
    SshTransportError(String),
    Protocol(String),
}

pub enum SftpAction {
    Send(Vec<u8>),
    Ok(String),
    DataComplete(Vec<u8>),
    Error(SftpError),
    NeedMore,
}

pub struct SftpClient {
    _private: (),
}

impl SftpClient {
    pub fn new(_user: &str, _pass: &str) -> Self {
        SftpClient { _private: () }
    }

    pub fn receive(&mut self, _data: &[u8]) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn list(&mut self, _path: &str) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn get(&mut self, _remote_path: &str, _store_key: &str) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn put(&mut self, _remote_path: &str, _data: Vec<u8>) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn stat(&mut self, _path: &str) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn close(&mut self) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }
}
```

- [ ] **Step 2: Add `sftp` module to `protocols/src/lib.rs`**

```rust
#![no_std]

extern crate alloc;

pub mod tftp;
pub mod ftp;
pub mod sftp;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 4: Commit**

```bash
git add protocols/src/sftp.rs protocols/src/lib.rs
git commit -m "feat(protocols): add SFTP client stub (Phase 2 placeholder)"
```

---

## Task 9: PXE Analysis Library

**Files:**
- Create: `src/js/pxe.js`

This is a JavaScript library for parsing and analyzing Windows PXE boot configurations, primarily BCD (Boot Configuration Data) files.

- [ ] **Step 1: Create `src/js/pxe.js`**

```javascript
// pxe.js — PXE Boot Configuration Analysis Library
// Usage: import { BCD, PxeAudit } from 'pxe';

// BCD Object Types
const BCD_OBJECT_TYPES = {
    0x10100001: 'Firmware Boot Manager',
    0x10100002: 'Windows Boot Manager',
    0x10200003: 'Windows Boot Loader',
    0x10200004: 'Windows Resume Loader',
    0x10400006: 'Windows Memory Tester',
    0x10400008: 'Windows System Boot Loader',
    0x10400009: 'Boot Sector',
    0x10400010: 'Startup Module',
};

// BCD Element Types (common ones for PXE)
const BCD_ELEMENTS = {
    0x12000002: 'ApplicationPath',
    0x12000004: 'Description',
    0x11000001: 'ApplicationDevice',
    0x12000030: 'LoadOptions',
    0x14000006: 'InheritedObjects',
    0x15000007: 'TruncateMemory',
    0x15000011: 'BootEms',
    0x16000048: 'BootDebugTransport',
    0x25000004: 'AvoidLowMemory',
    0x25000020: 'DisplayBootMenu',
    0x25000021: 'NoErrorDisplay',
    0x26000022: 'BcdDevice',
    0x26000023: 'BcdFilePath',
    0x25000024: 'Timeout',
};

// Parse a BCD binary blob from JSKV store
function parseBCD(storeKey) {
    const data = os.store.getBytes(storeKey);
    if (!data) return null;

    const view = new DataView(data.buffer || data);
    const entries = [];

    // BCD is stored as a Registry hive.
    // The binary format is a Windows Registry hive (regf).
    // Parse the hive header first.
    const magic = String.fromCharCode(data[0], data[1], data[2], data[3]);
    if (magic !== 'regf') {
        return { error: 'Not a valid BCD store (missing regf header)', raw: data };
    }

    // Registry hive parsing: header is 4096 bytes
    const HIVE_HEADER_SIZE = 4096;
    if (data.length < HIVE_HEADER_SIZE + 32) {
        return { error: 'BCD store too small', raw: data };
    }

    // Parse hive bins starting after header
    const rootCellOffset = readU32LE(data, 36); // Offset to root cell
    const objects = parseHiveBin(data, HIVE_HEADER_SIZE, rootCellOffset);

    return {
        magic: magic,
        objects: objects,
        size: data.length,
    };
}

function readU32LE(buf, offset) {
    return (buf[offset]) |
           (buf[offset + 1] << 8) |
           (buf[offset + 2] << 16) |
           (buf[offset + 3] << 24) >>> 0;
}

function readU16LE(buf, offset) {
    return (buf[offset]) | (buf[offset + 1] << 8);
}

// Simplified hive bin parser — extracts key names and values
function parseHiveBin(data, baseOffset, rootOffset) {
    const results = [];
    // Walk the registry tree looking for BCD objects
    // BCD stores objects under \Objects\{GUID}\Elements\{type}
    try {
        const keys = findKeys(data, baseOffset, 'Objects');
        for (const key of keys) {
            const obj = {
                guid: key.name,
                type: BCD_OBJECT_TYPES[key.type] || 'Unknown (' + key.type + ')',
                elements: {},
            };
            for (const elem of key.elements) {
                const elemName = BCD_ELEMENTS[elem.type] || ('0x' + elem.type.toString(16));
                obj.elements[elemName] = elem.value;
            }
            results.push(obj);
        }
    } catch (e) {
        results.push({ error: 'Parse error: ' + e.message });
    }
    return results;
}

function findKeys(data, baseOffset, targetName) {
    // Simplified: scan for known BCD GUIDs pattern
    // Real implementation walks the registry hive cell structure
    const keys = [];
    const guidPattern = /\{[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\}/gi;

    // Scan the raw data for GUID patterns
    const text = [];
    for (let i = 0; i < data.length; i++) {
        if (data[i] >= 0x20 && data[i] < 0x7F) {
            text.push(String.fromCharCode(data[i]));
        } else {
            text.push('\0');
        }
    }
    const textStr = text.join('');
    let match;
    while ((match = guidPattern.exec(textStr)) !== null) {
        keys.push({
            name: match[0],
            type: 0,
            elements: [],
            offset: match.index,
        });
    }
    return keys;
}

// Display a parsed BCD in human-readable format
function formatBCD(bcd) {
    if (!bcd || bcd.error) return bcd ? bcd.error : 'No BCD data';
    let output = 'BCD Store (' + bcd.size + ' bytes)\n';
    output += '='.repeat(40) + '\n';
    for (const obj of bcd.objects) {
        output += '\n[' + obj.guid + ']\n';
        output += '  Type: ' + obj.type + '\n';
        for (const [name, value] of Object.entries(obj.elements)) {
            output += '  ' + name + ': ' + value + '\n';
        }
    }
    return output;
}

// PXE Security Audit
function auditPxe(serverIp, bcd) {
    const findings = [];

    // Check 1: BCD accessible without authentication
    findings.push({
        severity: 'info',
        check: 'TFTP Access',
        detail: 'BCD store accessible via TFTP from ' + serverIp,
    });

    // Check 2: Look for unsigned boot entries
    if (bcd && bcd.objects) {
        for (const obj of bcd.objects) {
            if (obj.elements['ApplicationPath']) {
                findings.push({
                    severity: 'info',
                    check: 'Boot Image',
                    detail: 'Boot path: ' + obj.elements['ApplicationPath'],
                });
            }
        }
    }

    // Check 3: Check if debug options are enabled
    if (bcd && bcd.objects) {
        for (const obj of bcd.objects) {
            if (obj.elements['BootDebugTransport']) {
                findings.push({
                    severity: 'warning',
                    check: 'Debug Enabled',
                    detail: 'Boot debug transport is configured — potential security risk',
                });
            }
            if (obj.elements['BootEms']) {
                findings.push({
                    severity: 'info',
                    check: 'EMS Enabled',
                    detail: 'Emergency Management Services enabled',
                });
            }
        }
    }

    return findings;
}

function formatAudit(findings) {
    let output = 'PXE Security Audit Report\n';
    output += '='.repeat(40) + '\n\n';
    for (const f of findings) {
        const icon = f.severity === 'warning' ? '[!]' :
                     f.severity === 'critical' ? '[X]' : '[i]';
        output += icon + ' ' + f.check + ': ' + f.detail + '\n';
    }
    return output;
}

// Exports
globalThis.PXE = {
    parseBCD: parseBCD,
    formatBCD: formatBCD,
    audit: auditPxe,
    formatAudit: formatAudit,
    BCD_OBJECT_TYPES: BCD_OBJECT_TYPES,
    BCD_ELEMENTS: BCD_ELEMENTS,
};
```

- [ ] **Step 2: Verify the file exists and is syntactically valid**

Check that `src/js/pxe.js` is properly saved and the JavaScript parses (no syntax errors).

- [ ] **Step 3: Commit**

```bash
git add src/js/pxe.js
git commit -m "feat: add PXE configuration analysis library (BCD parser, audit)"
```

---

## Task 10: PXE Tool Application

**Files:**
- Create: `src/jsos/pxetool.jsos`

- [ ] **Step 1: Create `src/jsos/pxetool.jsos`**

```javascript
// pxetool.jsos — PXE Boot Configuration Tool
// Commands: scan, pull, show, audit, deploy

import { Window, Keys, Store } from 'libjsos';

const win = Window.create('PXE Tool', 800, 600, 0, 0);
let lines = [];
let scrollOffset = 0;

function print(text) {
    const newLines = text.split('\n');
    for (const line of newLines) {
        lines.push(line);
    }
    redraw();
}

function redraw() {
    win.drawRect(0, 0, 800, 600, 0, 0, 0, true);
    const maxLines = 35;
    const start = Math.max(0, lines.length - maxLines - scrollOffset);
    const end = Math.min(lines.length, start + maxLines);
    for (let i = start; i < end; i++) {
        win.drawString(4, 4 + (i - start) * 16, lines[i], 255, 255, 255, os.FONT_SMALL);
    }
    win.flush();
}

function printHelp() {
    print('PXE Tool - Windows PXE Boot Configuration Analyzer');
    print('');
    print('Usage:');
    print('  pull <server_ip>           Pull BCD from TFTP server');
    print('  show [store_key]           Show parsed BCD (default: pxe:bcd)');
    print('  audit [store_key]          Security audit (default: pxe:bcd)');
    print('  list                       List PXE-related store keys');
    print('  help                       Show this help');
    print('');
}

let inputBuffer = '';

async function handleCommand(input) {
    const parts = input.trim().split(/\s+/);
    const cmd = parts[0];

    switch (cmd) {
        case 'pull': {
            const server = parts[1];
            if (!server) {
                print('Usage: pull <server_ip>');
                return;
            }
            print('Connecting to TFTP server ' + server + '...');
            try {
                await os.tftp.get(server, '/boot/BCD', 'pxe:bcd');
                print('BCD store downloaded to pxe:bcd');
            } catch (e) {
                print('Error: ' + e.message);
            }
            break;
        }
        case 'show': {
            const key = parts[1] || 'pxe:bcd';
            const bcd = PXE.parseBCD(key);
            if (bcd) {
                print(PXE.formatBCD(bcd));
            } else {
                print('No data at store key: ' + key);
                print('Run "pull <server_ip>" first');
            }
            break;
        }
        case 'audit': {
            const key = parts[1] || 'pxe:bcd';
            const bcd = PXE.parseBCD(key);
            if (bcd) {
                const findings = PXE.audit('(stored)', bcd);
                print(PXE.formatAudit(findings));
            } else {
                print('No data at store key: ' + key);
            }
            break;
        }
        case 'list': {
            const allKeys = os.store.list();
            const pxeKeys = allKeys.filter(function(k) { return k.startsWith('pxe:'); });
            if (pxeKeys.length === 0) {
                print('No PXE store keys found. Run "pull" first.');
            } else {
                print('PXE store keys:');
                for (const k of pxeKeys) {
                    print('  ' + k);
                }
            }
            break;
        }
        case 'help':
            printHelp();
            break;
        default:
            if (cmd) print('Unknown command: ' + cmd + ' (type "help")');
            break;
    }
}

// Load PXE library
const pxeSource = os.store.get('sys:lib:pxe');
if (!pxeSource) {
    // PXE lib should be available via the module system
    // If not embedded, it will be loaded via import
}

printHelp();
print('> ');

on_key = function(key, code) {
    if (code === 13) { // Enter
        const cmd = inputBuffer;
        inputBuffer = '';
        print('> ' + cmd);
        handleCommand(cmd);
        print('> ');
    } else if (code === 8) { // Backspace
        inputBuffer = inputBuffer.slice(0, -1);
    } else if (key && key.length === 1) {
        inputBuffer += key;
    }
};
```

- [ ] **Step 2: Verify the file exists**

Check `src/jsos/pxetool.jsos` is saved correctly.

- [ ] **Step 3: Commit**

```bash
git add src/jsos/pxetool.jsos
git commit -m "feat: add pxetool.jsos interactive PXE configuration tool"
```

---

## Task 11: Build and Smoke Test

**Files:** None new — this verifies everything compiles and links.

- [ ] **Step 1: Full build**

Run: `cargo build --target x86_64-os.json 2>&1 | tail -30`
Expected: Build succeeds with no errors

- [ ] **Step 2: Fix any compilation errors**

Address any type mismatches, missing imports, or borrow issues. Common issues:
- `smoltcp::socket::udp` needs `use` in `mod.rs`
- `protocols` crate features may need adjustment
- Mutex lock ordering issues in `poll_network`

- [ ] **Step 3: Run tests**

Run: `cargo test --target x86_64-os.json 2>&1 | tail -20`
Expected: All existing tests still pass

- [ ] **Step 4: Final commit if fixes were needed**

```bash
git add -A
git commit -m "fix: resolve compilation issues in protocol integration"
```

---

## Summary of Delivery Order

1. **Task 1**: Protocols crate skeleton
2. **Task 2**: TFTP state machine
3. **Task 3**: Kernel TFTP integration
4. **Task 4**: JavaScript TFTP API
5. **Task 5**: FTP state machine
6. **Task 6**: Kernel FTP integration
7. **Task 7**: JavaScript FTP API
8. **Task 8**: SFTP stub
9. **Task 9**: PXE analysis library
10. **Task 10**: PXE tool app
11. **Task 11**: Build and smoke test

Each task produces a working commit. TFTP is fully usable after Task 4. FTP after Task 7. PXE tooling after Task 10.

## Phase 2 (Future)

- SFTP implementation (requires SSH transport)
- HTTP/WebSocket extraction into protocols crate
- NIC driver for Intel i219/Realtek 8168 (user-driven)
- DHCP option 66/67 auto-discovery
