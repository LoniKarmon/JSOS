// FTP client state machine — RFC 959
// Pure protocol logic: no sockets, no I/O. Produces/consumes byte buffers.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum FtpError {
    AuthFailed,
    ConnectionClosed,
    ServerError(String),
    Protocol(String),
    InvalidPassiveResponse,
}

// ---------------------------------------------------------------------------
// Action type
// ---------------------------------------------------------------------------

pub enum FtpAction {
    /// Send bytes on the control channel TCP connection.
    SendControl(Vec<u8>),
    /// Open a new TCP connection to (ipv4_as_u32, port) for the data channel.
    ConnectData(u32, u16),
    /// Send bytes on the data channel TCP connection (used for STOR).
    SendData(Vec<u8>),
    /// File download complete; here is the assembled data.
    DataComplete(Vec<u8>),
    /// Command succeeded with a server message.
    Ok(String),
    /// An error occurred.
    Error(FtpError),
    /// Need more data; nothing to do this tick.
    NeedMore,
}

// ---------------------------------------------------------------------------
// Pending command (stashed while TYPE I / PASV handshake proceeds)
// ---------------------------------------------------------------------------

enum PendingCommand {
    List(String),
    Retr(String),
    Stor(String, Vec<u8>),
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

enum State {
    /// Waiting for the server's 220 greeting.
    WaitGreeting,
    /// USER sent; waiting for 331.
    WaitUserOk,
    /// PASS sent; waiting for 230.
    WaitPassOk,
    /// Logged in and ready for commands.
    Ready,
    /// TYPE I sent; waiting for 200.
    WaitTypeOk,
    /// PASV sent; waiting for 227.
    WaitPasv,
    /// ConnectData returned; waiting for the TCP data channel to reach
    /// Established (data_connected() callback).
    WaitDataConnect,
    /// Data channel connected; the actual LIST/RETR/STOR command sent,
    /// waiting for 150.
    WaitTransferStart,
    /// 150 received; accumulating data channel bytes / waiting for 226.
    Transferring,
    /// QUIT sent; session over.
    Done,
    /// Terminal error state.
    Failed,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct FtpClient {
    state: State,
    /// Credentials.
    user: String,
    pass: String,
    /// Whether TYPE I has been sent successfully this session.
    type_set: bool,
    /// The command to execute once PASV + data-connect succeed.
    pending: Option<PendingCommand>,
    /// Accumulated control-channel bytes that have not yet formed a complete line.
    ctrl_buf: Vec<u8>,
    /// Accumulated data-channel bytes.
    data_buf: Vec<u8>,
}

impl FtpClient {
    // -----------------------------------------------------------------------
    // Constructor
    // -----------------------------------------------------------------------

    pub fn new(user: &str, pass: &str) -> Self {
        FtpClient {
            state: State::WaitGreeting,
            user: user.to_string(),
            pass: pass.to_string(),
            type_set: false,
            pending: None,
            ctrl_buf: Vec::new(),
            data_buf: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Public query helpers
    // -----------------------------------------------------------------------

    /// Returns the well-known FTP control port.
    pub fn server_port() -> u16 {
        21
    }

    /// True when logged in and ready to accept commands.
    pub fn is_ready(&self) -> bool {
        matches!(self.state, State::Ready)
    }

    /// True when the session has ended (QUIT sent or fatal error).
    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Done | State::Failed)
    }

    // -----------------------------------------------------------------------
    // Control channel input
    // -----------------------------------------------------------------------

    /// Feed bytes received on the control channel. Returns the next action.
    pub fn receive_control(&mut self, data: &[u8]) -> FtpAction {
        self.ctrl_buf.extend_from_slice(data);

        // Extract and process one complete line at a time. If there are
        // multiple lines buffered (server burst), process only the first; the
        // caller must call receive_control again (or we could loop, but a
        // single-action-per-call model is simpler for the kernel driver).
        // We iterate until no complete line is available.
        loop {
            match self.take_line() {
                None => return FtpAction::NeedMore,
                Some(line) => {
                    let action = self.handle_line(&line);
                    // If the action is NeedMore we keep processing buffered lines.
                    match action {
                        FtpAction::NeedMore => continue,
                        other => return other,
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Data channel callbacks
    // -----------------------------------------------------------------------

    /// Feed bytes received on the data channel.
    pub fn receive_data(&mut self, data: &[u8]) -> FtpAction {
        self.data_buf.extend_from_slice(data);
        FtpAction::NeedMore
    }

    /// Called when the server closes the data channel TCP connection.
    pub fn data_channel_closed(&mut self) -> FtpAction {
        match self.state {
            State::Transferring | State::WaitTransferStart => {
                // Transfer is complete.
                self.state = State::Ready;
                let data = core::mem::take(&mut self.data_buf);
                FtpAction::DataComplete(data)
            }
            _ => FtpAction::NeedMore,
        }
    }

    /// Called when the data channel TCP connection reaches Established.
    pub fn data_connected(&mut self) -> FtpAction {
        match self.state {
            State::WaitDataConnect => {
                // Send the stashed LIST/RETR/STOR command.
                match self.pending.take() {
                    Some(PendingCommand::List(path)) => {
                        self.state = State::WaitTransferStart;
                        let cmd = ftp_cmd("LIST", &path);
                        FtpAction::SendControl(cmd)
                    }
                    Some(PendingCommand::Retr(path)) => {
                        self.state = State::WaitTransferStart;
                        let cmd = ftp_cmd("RETR", &path);
                        FtpAction::SendControl(cmd)
                    }
                    Some(PendingCommand::Stor(path, upload_data)) => {
                        self.state = State::WaitTransferStart;
                        let cmd = ftp_cmd("STOR", &path);
                        // We need to send the STOR command first, then SendData.
                        // Since we can only return one action, we stash the
                        // upload data in a temporary PendingCommand::Stor with
                        // an empty path to signal "awaiting 150 for upload".
                        self.pending = Some(PendingCommand::Stor(String::new(), upload_data));
                        FtpAction::SendControl(cmd)
                    }
                    None => FtpAction::Error(FtpError::Protocol(
                        String::from("data_connected but no pending command"),
                    )),
                }
            }
            _ => FtpAction::NeedMore,
        }
    }

    // -----------------------------------------------------------------------
    // User-issued commands (only valid when is_ready())
    // -----------------------------------------------------------------------

    pub fn list(&mut self, path: &str) -> FtpAction {
        self.start_transfer(PendingCommand::List(path.to_string()))
    }

    pub fn get(&mut self, remote_path: &str) -> FtpAction {
        self.start_transfer(PendingCommand::Retr(remote_path.to_string()))
    }

    pub fn put(&mut self, remote_path: &str, data: Vec<u8>) -> FtpAction {
        self.start_transfer(PendingCommand::Stor(remote_path.to_string(), data))
    }

    pub fn mkdir(&mut self, path: &str) -> FtpAction {
        if !matches!(self.state, State::Ready) {
            return FtpAction::Error(FtpError::Protocol(String::from("not ready")));
        }
        // MKD does not need PASV / data channel.
        FtpAction::SendControl(ftp_cmd("MKD", path))
    }

    pub fn delete(&mut self, path: &str) -> FtpAction {
        if !matches!(self.state, State::Ready) {
            return FtpAction::Error(FtpError::Protocol(String::from("not ready")));
        }
        // DELE does not need PASV / data channel.
        FtpAction::SendControl(ftp_cmd("DELE", path))
    }

    pub fn quit(&mut self) -> FtpAction {
        if !matches!(self.state, State::Ready) {
            return FtpAction::Error(FtpError::Protocol(String::from("not ready")));
        }
        self.state = State::Done;
        FtpAction::SendControl(b"QUIT\r\n".to_vec())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Begin a transfer: set TYPE if needed, otherwise send PASV.
    fn start_transfer(&mut self, cmd: PendingCommand) -> FtpAction {
        if !matches!(self.state, State::Ready) {
            return FtpAction::Error(FtpError::Protocol(String::from("not ready")));
        }
        self.pending = Some(cmd);
        if !self.type_set {
            self.state = State::WaitTypeOk;
            FtpAction::SendControl(b"TYPE I\r\n".to_vec())
        } else {
            self.state = State::WaitPasv;
            FtpAction::SendControl(b"PASV\r\n".to_vec())
        }
    }

    /// Extract one complete `\r\n`-terminated line from `ctrl_buf`. Returns
    /// `None` if no complete line is available yet.
    fn take_line(&mut self) -> Option<String> {
        // Look for \n (covers both \r\n and bare \n).
        let pos = self.ctrl_buf.iter().position(|&b| b == b'\n')?;
        let raw: Vec<u8> = self.ctrl_buf.drain(..=pos).collect();
        // Strip trailing \r\n / \n and convert to String.
        let trimmed = raw
            .iter()
            .copied()
            .filter(|&b| b != b'\r' && b != b'\n')
            .collect::<Vec<u8>>();
        Some(String::from_utf8_lossy(&trimmed).into_owned())
    }

    /// Dispatch a single control-channel line through the state machine.
    fn handle_line(&mut self, line: &str) -> FtpAction {
        // FTP multi-line responses: "NNN-..." continues, "NNN ..." ends.
        // We ignore continuation lines and only act on the final (space) line.
        let code = response_code(line);
        let is_final = line.len() >= 4 && line.as_bytes().get(3) == Some(&b' ');

        if !is_final {
            // Continuation line of a multi-line response — skip.
            return FtpAction::NeedMore;
        }

        match self.state {
            State::WaitGreeting => {
                if code == 220 {
                    // Send USER command.
                    let user = self.user.clone();
                    self.state = State::WaitUserOk;
                    let cmd = ftp_cmd("USER", &user);
                    FtpAction::SendControl(cmd)
                } else {
                    self.fail(FtpError::ServerError(line.to_string()))
                }
            }

            State::WaitUserOk => {
                if code == 331 {
                    let pass = self.pass.clone();
                    self.state = State::WaitPassOk;
                    let cmd = ftp_cmd("PASS", &pass);
                    FtpAction::SendControl(cmd)
                } else {
                    self.fail(FtpError::AuthFailed)
                }
            }

            State::WaitPassOk => {
                if code == 230 {
                    self.state = State::Ready;
                    FtpAction::Ok(line.to_string())
                } else {
                    self.fail(FtpError::AuthFailed)
                }
            }

            State::Ready => {
                // Responses to MKD / DELE arrive here.
                match code {
                    257 => FtpAction::Ok(line.to_string()), // MKD ok
                    250 => FtpAction::Ok(line.to_string()), // DELE ok
                    221 => {
                        self.state = State::Done;
                        FtpAction::Ok(line.to_string()) // QUIT reply
                    }
                    _ => FtpAction::Error(FtpError::ServerError(line.to_string())),
                }
            }

            State::WaitTypeOk => {
                if code == 200 {
                    self.type_set = true;
                    self.state = State::WaitPasv;
                    FtpAction::SendControl(b"PASV\r\n".to_vec())
                } else {
                    self.fail(FtpError::ServerError(line.to_string()))
                }
            }

            State::WaitPasv => {
                if code == 227 {
                    match parse_pasv(line) {
                        Some((ip, port)) => {
                            self.state = State::WaitDataConnect;
                            FtpAction::ConnectData(ip, port)
                        }
                        None => self.fail(FtpError::InvalidPassiveResponse),
                    }
                } else {
                    self.fail(FtpError::ServerError(line.to_string()))
                }
            }

            State::WaitDataConnect => {
                // We should not be receiving control data while waiting for the
                // TCP data connection to establish — ignore / buffer.
                FtpAction::NeedMore
            }

            State::WaitTransferStart => {
                if code == 150 || code == 125 {
                    self.state = State::Transferring;
                    // For STOR: now that the server is ready, send the upload data.
                    if let Some(PendingCommand::Stor(_, ref upload_data)) = self.pending {
                        let data = upload_data.clone();
                        self.pending = None;
                        return FtpAction::SendData(data);
                    }
                    FtpAction::NeedMore
                } else {
                    self.fail(FtpError::ServerError(line.to_string()))
                }
            }

            State::Transferring => {
                // 226 = Transfer complete; 250 = also acceptable.
                if code == 226 || code == 250 {
                    self.state = State::Ready;
                    let data = core::mem::take(&mut self.data_buf);
                    FtpAction::DataComplete(data)
                } else {
                    self.fail(FtpError::ServerError(line.to_string()))
                }
            }

            State::Done | State::Failed => FtpAction::NeedMore,
        }
    }

    /// Transition to Failed and return an Error action.
    fn fail(&mut self, err: FtpError) -> FtpAction {
        self.state = State::Failed;
        FtpAction::Error(err)
    }
}

// ---------------------------------------------------------------------------
// Protocol helpers
// ---------------------------------------------------------------------------

/// Extract the 3-digit numeric response code from a line. Returns 0 on error.
fn response_code(line: &str) -> u16 {
    if line.len() < 3 {
        return 0;
    }
    line[..3].parse::<u16>().unwrap_or(0)
}

/// Build an FTP command line: `VERB arg\r\n`.
/// If `arg` is empty, just `VERB\r\n`.
fn ftp_cmd(verb: &str, arg: &str) -> Vec<u8> {
    let mut s = String::from(verb);
    if !arg.is_empty() {
        s.push(' ');
        s.push_str(arg);
    }
    s.push_str("\r\n");
    s.into_bytes()
}

/// Parse the PASV 227 response, e.g.:
/// "227 Entering Passive Mode (192,168,1,1,12,34)"
/// Returns (ipv4_as_u32, port) or None on parse failure.
fn parse_pasv(line: &str) -> Option<(u32, u16)> {
    // Find the opening paren.
    let start = line.find('(')?;
    let end = line.find(')')?;
    if end <= start {
        return None;
    }
    let inner = &line[start + 1..end];
    let parts: Vec<u16> = inner
        .split(',')
        .filter_map(|s| s.trim().parse::<u16>().ok())
        .collect();
    if parts.len() != 6 {
        return None;
    }
    let ip: u32 = ((parts[0] as u32) << 24)
        | ((parts[1] as u32) << 16)
        | ((parts[2] as u32) << 8)
        | (parts[3] as u32);
    let port: u16 = parts[4] * 256 + parts[5];
    Some((ip, port))
}
