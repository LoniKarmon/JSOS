// TFTP client state machine — RFC 1350
// Pure protocol logic: no sockets, no I/O. Produces/consumes byte buffers.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// Opcodes
const OP_RRQ: u16 = 1;
const OP_WRQ: u16 = 2;
const OP_DATA: u16 = 3;
const OP_ACK: u16 = 4;
const OP_ERROR: u16 = 5;

const BLOCK_SIZE: usize = 512;
pub const TFTP_PORT: u16 = 69;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

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

impl TftpError {
    /// Map a TFTP error code (RFC 1350 §5) to a TftpError variant.
    fn from_code(code: u16, msg: String) -> Self {
        match code {
            1 => TftpError::FileNotFound,
            2 => TftpError::AccessViolation,
            3 => TftpError::DiskFull,
            4 => TftpError::IllegalOperation,
            5 => TftpError::UnknownTransferId,
            6 => TftpError::FileExists,
            _ => TftpError::Protocol(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Action type
// ---------------------------------------------------------------------------

pub enum TftpAction {
    /// Send this UDP payload to the server.
    Send(Vec<u8>),
    /// Transfer is complete and the assembled file data is ready.
    /// The caller should also send the final ACK, which is separately
    /// available via `take_data()` after inspecting `is_complete()`.
    /// (See design note in task description — we use Send + is_complete.)
    Complete(Vec<u8>),
    /// An error occurred.
    Error(TftpError),
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

enum State {
    /// After RRQ sent: waiting for first DATA block (block #1).
    WaitingData,
    /// Receiving data blocks; we know which block number we expect next.
    Receiving { expected_block: u16 },
    /// After WRQ or DATA sent: waiting for ACK.
    WaitingAck { next_block: u16 },
    /// Transfer complete.
    Done,
    /// Terminal error state.
    Failed,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct TftpClient {
    state: State,
    /// Accumulated received data (read direction).
    rx_buf: Vec<u8>,
    /// Data to send (write direction): blocks are sliced from here.
    tx_data: Vec<u8>,
}

impl TftpClient {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Begin a read transfer. Returns `(client, RRQ_packet)`.
    pub fn start_read(filename: &str) -> (Self, Vec<u8>) {
        let pkt = build_rrq(filename);
        let client = TftpClient {
            state: State::WaitingData,
            rx_buf: Vec::new(),
            tx_data: Vec::new(),
        };
        (client, pkt)
    }

    /// Begin a write transfer. Returns `(client, WRQ_packet)`.
    pub fn start_write(filename: &str, data: Vec<u8>) -> (Self, Vec<u8>) {
        let pkt = build_wrq(filename);
        let client = TftpClient {
            state: State::WaitingAck { next_block: 1 },
            rx_buf: Vec::new(),
            tx_data: data,
        };
        (client, pkt)
    }

    // -----------------------------------------------------------------------
    // Public query helpers
    // -----------------------------------------------------------------------

    /// Returns the well-known TFTP server port (initial request port).
    pub fn server_port() -> u16 {
        TFTP_PORT
    }

    /// True once the transfer has finished successfully.
    pub fn is_complete(&self) -> bool {
        matches!(self.state, State::Done)
    }

    /// Take ownership of the assembled received data. Only meaningful after
    /// `is_complete()` returns true.  Clears the internal buffer.
    pub fn take_data(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.rx_buf)
    }

    // -----------------------------------------------------------------------
    // State machine driver
    // -----------------------------------------------------------------------

    /// Feed a UDP packet received from the server; returns the next action.
    pub fn receive(&mut self, packet: &[u8]) -> TftpAction {
        if packet.len() < 2 {
            return TftpAction::Error(TftpError::Protocol(
                String::from("packet too short"),
            ));
        }

        let opcode = u16::from_be_bytes([packet[0], packet[1]]);

        // Handle ERROR packets from any state.
        if opcode == OP_ERROR {
            return self.handle_error(packet);
        }

        match opcode {
            OP_DATA => self.handle_data(packet),
            OP_ACK => self.handle_ack(packet),
            _ => TftpAction::Error(TftpError::Protocol(
                String::from("unexpected opcode"),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Packet handlers
    // -----------------------------------------------------------------------

    fn handle_data(&mut self, packet: &[u8]) -> TftpAction {
        if packet.len() < 4 {
            return TftpAction::Error(TftpError::Protocol(
                String::from("DATA packet too short"),
            ));
        }
        let block = u16::from_be_bytes([packet[2], packet[3]]);
        let payload = &packet[4..];

        let expected = match self.state {
            State::WaitingData => 1u16,
            State::Receiving { expected_block } => expected_block,
            _ => {
                return TftpAction::Error(TftpError::Protocol(
                    String::from("unexpected DATA in current state"),
                ));
            }
        };

        if block != expected {
            // Duplicate or out-of-order block — re-ACK the previous block.
            let prev = expected.wrapping_sub(1);
            return TftpAction::Send(build_ack(prev));
        }

        // Append payload.
        self.rx_buf.extend_from_slice(payload);

        let is_last = payload.len() < BLOCK_SIZE;

        if is_last {
            // Transition to Done before returning so is_complete() is true
            // immediately after the caller processes the Send action.
            self.state = State::Done;
            TftpAction::Send(build_ack(block))
        } else {
            self.state = State::Receiving {
                expected_block: block.wrapping_add(1),
            };
            TftpAction::Send(build_ack(block))
        }
    }

    fn handle_ack(&mut self, packet: &[u8]) -> TftpAction {
        if packet.len() < 4 {
            return TftpAction::Error(TftpError::Protocol(
                String::from("ACK packet too short"),
            ));
        }
        let ack_block = u16::from_be_bytes([packet[2], packet[3]]);

        let next_block = match self.state {
            State::WaitingAck { next_block } => next_block,
            _ => {
                return TftpAction::Error(TftpError::Protocol(
                    String::from("unexpected ACK in current state"),
                ));
            }
        };

        // ACK 0 is the server's acknowledgment of our WRQ.
        // ACK N acknowledges DATA block N; we should send block N+1.
        let expected_ack = next_block.wrapping_sub(1);
        if ack_block != expected_ack {
            // Ignore unexpected ACK (could re-send current block, but we
            // simply wait for the correct ACK to avoid duplicate sends).
            return TftpAction::Send(self.current_data_block(next_block));
        }

        // Build the next DATA block.
        let block_index = (next_block as usize) - 1; // 0-based
        let start = block_index * BLOCK_SIZE;

        if start >= self.tx_data.len() && next_block > 1 {
            // All data has been ACK'd — transfer complete.
            self.state = State::Done;
            return TftpAction::Complete(Vec::new());
        }

        let end = core::cmp::min(start + BLOCK_SIZE, self.tx_data.len());
        let chunk = &self.tx_data[start..end];
        let data_pkt = build_data(next_block, chunk);
        let is_last = chunk.len() < BLOCK_SIZE;

        if is_last {
            // After sending the last (possibly empty) block, we wait for
            // the final ACK before declaring Done.
            self.state = State::WaitingAck {
                next_block: next_block.wrapping_add(1),
            };
        } else {
            self.state = State::WaitingAck {
                next_block: next_block.wrapping_add(1),
            };
        }

        TftpAction::Send(data_pkt)
    }

    fn handle_error(&mut self, packet: &[u8]) -> TftpAction {
        self.state = State::Failed;
        if packet.len() < 4 {
            return TftpAction::Error(TftpError::Protocol(
                String::from("ERROR packet too short"),
            ));
        }
        let code = u16::from_be_bytes([packet[2], packet[3]]);
        let msg = if packet.len() > 4 {
            // Null-terminated string starting at byte 4.
            let raw = &packet[4..];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        } else {
            String::new()
        };
        TftpAction::Error(TftpError::from_code(code, msg))
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Build the DATA packet for `block` from `self.tx_data`.
    fn current_data_block(&self, block: u16) -> Vec<u8> {
        let start = (block as usize - 1) * BLOCK_SIZE;
        let end = core::cmp::min(start + BLOCK_SIZE, self.tx_data.len());
        if start > self.tx_data.len() {
            return build_data(block, &[]);
        }
        build_data(block, &self.tx_data[start..end])
    }
}

// ---------------------------------------------------------------------------
// Packet builders
// ---------------------------------------------------------------------------

/// Build an RRQ (read request) packet: opcode + filename\0 + "octet"\0
fn build_rrq(filename: &str) -> Vec<u8> {
    build_request(OP_RRQ, filename)
}

/// Build a WRQ (write request) packet: opcode + filename\0 + "octet"\0
fn build_wrq(filename: &str) -> Vec<u8> {
    build_request(OP_WRQ, filename)
}

fn build_request(opcode: u16, filename: &str) -> Vec<u8> {
    let mut pkt = Vec::new();
    let op = opcode.to_be_bytes();
    pkt.push(op[0]);
    pkt.push(op[1]);
    pkt.extend_from_slice(filename.as_bytes());
    pkt.push(0); // null terminator
    pkt.extend_from_slice(b"octet");
    pkt.push(0); // null terminator
    pkt
}

/// Build an ACK packet: opcode(4) + block_number
fn build_ack(block: u16) -> Vec<u8> {
    let op = OP_ACK.to_be_bytes();
    let blk = block.to_be_bytes();
    alloc::vec![op[0], op[1], blk[0], blk[1]]
}

/// Build a DATA packet: opcode(3) + block_number + payload
fn build_data(block: u16, payload: &[u8]) -> Vec<u8> {
    let op = OP_DATA.to_be_bytes();
    let blk = block.to_be_bytes();
    let mut pkt = Vec::with_capacity(4 + payload.len());
    pkt.push(op[0]);
    pkt.push(op[1]);
    pkt.push(blk[0]);
    pkt.push(blk[1]);
    pkt.extend_from_slice(payload);
    pkt
}
