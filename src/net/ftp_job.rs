// src/net/ftp_job.rs — FTP session: wires FtpClient state machine to smoltcp TCP sockets.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp::{self, Socket as TcpSocket, State};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use protocols::ftp::{FtpClient, FtpAction};
use crate::serial_println;
use crate::storage;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpJobState {
    /// TCP connect() has been called; waiting for smoltcp to send SYN.
    Connecting,
    /// SYN sent; waiting for Established.
    WaitEstablished,
    /// Control channel is up; exchanging FTP protocol messages.
    Active,
    /// Session is finished (QUIT sent or fatal error).
    Done,
}

pub enum FtpCommandKind {
    List(String),
    /// (remote_path, store_key)
    Get(String, String),
    /// (remote_path, store_key) — reads data from JSKV
    Put(String, String),
    Mkdir(String),
    Delete(String),
    Close,
}

pub struct FtpCommand {
    pub kind: FtpCommandKind,
    pub callback_id: String,
}

pub struct FtpResultItem {
    pub callback_id: String,
    pub success: bool,
    pub data: String,
}

pub struct FtpSession {
    pub pid: u32,
    pub session_id: u64,
    pub control_handle: SocketHandle,
    pub data_handle: Option<SocketHandle>,
    /// True once we have notified the FtpClient that the data channel is Established.
    pub data_connect_notified: bool,
    pub client: FtpClient,
    pub server_addr: IpAddress,
    pub start_ticks: u64,
    pub last_activity: u64,
    pub job_state: FtpJobState,
    pub command_queue: Vec<FtpCommand>,
    pub pending_results: Vec<FtpResultItem>,
    /// Store key for the currently in-flight GET command.
    pub current_get_store_key: Option<String>,
    /// Callback ID for the currently executing command (in-flight).
    pub current_callback_id: Option<String>,
    pub closed: bool,
}

// ---------------------------------------------------------------------------
// Session counter (simple monotonic ID)
// ---------------------------------------------------------------------------

static NEXT_SESSION_ID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(1);

// ---------------------------------------------------------------------------
// Create a new FTP session
// ---------------------------------------------------------------------------

/// Allocate a control-channel TCP socket, register the session, and return
/// the session ID.  TCP connect() is deferred to the first `poll_ftp_sessions`
/// call so that we have an `Interface` context available.
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

    let tcp_rx = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let tcp_tx = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let tcp_socket = TcpSocket::new(tcp_rx, tcp_tx);
    let control_handle = sockets.add(tcp_socket);

    let session_id = NEXT_SESSION_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let current_ticks = crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    let session = FtpSession {
        pid,
        session_id,
        control_handle,
        data_handle: None,
        data_connect_notified: false,
        client: FtpClient::new(user, pass),
        server_addr,
        start_ticks: current_ticks,
        last_activity: current_ticks,
        job_state: FtpJobState::Connecting,
        command_queue: Vec::new(),
        pending_results: Vec::new(),
        current_get_store_key: None,
        current_callback_id: None,
        closed: false,
    };

    super::FTP_SESSIONS.lock().insert(session_id, Box::new(session));
    serial_println!("[FTP] Session {} created for pid={} server={}", session_id, pid, server_ip);
    session_id as i64
}

// ---------------------------------------------------------------------------
// Enqueue a command on an existing session
// ---------------------------------------------------------------------------

pub fn enqueue_ftp_command(session_id: u64, cmd: FtpCommand) {
    let mut sessions = super::FTP_SESSIONS.lock();
    if let Some(session) = sessions.get_mut(&session_id) {
        session.command_queue.push(cmd);
    } else {
        serial_println!("[FTP] enqueue: session {} not found", session_id);
    }
}

// ---------------------------------------------------------------------------
// Main poll function — called from poll_network() each tick
// ---------------------------------------------------------------------------

pub fn poll_ftp_sessions(
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    current_ticks: u64,
) {
    // Collect session IDs to iterate without holding the lock the whole time.
    // We do need to re-lock for each session, but since this is single-threaded
    // and no interrupt handler touches FTP_SESSIONS that is fine.
    let session_ids: Vec<u64> = {
        let sessions = super::FTP_SESSIONS.lock();
        sessions.keys().copied().collect()
    };

    for session_id in session_ids {
        // Temporarily take ownership of the session so we can pass `sockets`
        // and `iface` to helpers without borrow conflicts.
        let mut session = {
            let mut sessions = super::FTP_SESSIONS.lock();
            match sessions.remove(&session_id) {
                Some(s) => s,
                None => continue,
            }
        };

        // Timeout: ~60 seconds (100 ticks/s * 60 = 6000)
        if current_ticks.saturating_sub(session.last_activity) > 6000 {
            serial_println!("[FTP] Session {} timed out", session_id);
            close_session_sockets(&mut session, sockets);
            session.closed = true;
            // Push error results for any waiting callbacks.
            push_pending_error(&mut session, String::from("Error: FTP Timeout"));
            collect_results(&session);
            // Don't re-insert — session is dropped.
            continue;
        }

        if !session.closed {
            poll_one_session(&mut session, sockets, iface, current_ticks);
        }

        // Re-insert unless fully closed and done.
        let keep = !session.closed;
        if keep {
            super::FTP_SESSIONS.lock().insert(session_id, session);
        } else {
            // Collect any remaining results before dropping.
            collect_results(&session);
        }
    }
}

// ---------------------------------------------------------------------------
// Poll a single session through its state machine
// ---------------------------------------------------------------------------

fn poll_one_session(
    session: &mut FtpSession,
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    current_ticks: u64,
) {
    match session.job_state {
        FtpJobState::Connecting => {
            // Issue the TCP connect now that we have an Interface context.
            let cx = iface.context();
            let local_port = super::next_local_port();
            let remote_endpoint = IpEndpoint::new(session.server_addr, FtpClient::server_port());
            let ctrl_socket = sockets.get_mut::<TcpSocket>(session.control_handle);
            match ctrl_socket.connect(cx, remote_endpoint, local_port) {
                Ok(()) => {
                    serial_println!("[FTP] Session {} connecting to {}:{}", session.session_id, session.server_addr, FtpClient::server_port());
                    session.job_state = FtpJobState::WaitEstablished;
                }
                Err(e) => {
                    serial_println!("[FTP] Session {} connect error: {:?}", session.session_id, e);
                    session.closed = true;
                    push_pending_error(session, format!("Error: TCP Connect Failed: {:?}", e));
                }
            }
        }

        FtpJobState::WaitEstablished => {
            let ctrl_socket = sockets.get_mut::<TcpSocket>(session.control_handle);
            match ctrl_socket.state() {
                State::Established => {
                    serial_println!("[FTP] Session {} control channel established", session.session_id);
                    session.job_state = FtpJobState::Active;
                    session.last_activity = current_ticks;
                }
                State::Closed | State::CloseWait => {
                    serial_println!("[FTP] Session {} connection refused", session.session_id);
                    session.closed = true;
                    push_pending_error(session, String::from("Error: TCP Connection Refused"));
                }
                _ => {} // SynSent — still waiting
            }
        }

        FtpJobState::Active => {
            session.last_activity = current_ticks;

            // --- Read data channel (if present) ---
            if let Some(data_handle) = session.data_handle {
                let data_state = sockets.get_mut::<TcpSocket>(data_handle).state();

                // Notify data_connected() on first Established.
                if data_state == State::Established && !session.data_connect_notified {
                    session.data_connect_notified = true;
                    let action = session.client.data_connected();
                    handle_ftp_action(session, sockets, iface, action);
                }

                // Read available data.
                {
                    let mut buf = [0u8; 2048];
                    loop {
                        let data_socket = sockets.get_mut::<TcpSocket>(data_handle);
                        match data_socket.recv_slice(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let action = session.client.receive_data(&buf[..n]);
                                handle_ftp_action(session, sockets, iface, action);
                            }
                        }
                    }
                }

                // Detect data channel closed by remote.
                let data_state_now = sockets.get_mut::<TcpSocket>(data_handle).state();
                if matches!(data_state_now, State::CloseWait | State::Closed | State::TimeWait | State::FinWait2) {
                    serial_println!("[FTP] Session {} data channel closed", session.session_id);
                    let action = session.client.data_channel_closed();
                    handle_ftp_action(session, sockets, iface, action);
                    // Remove data socket.
                    let data_socket = sockets.get_mut::<TcpSocket>(data_handle);
                    data_socket.close();
                    sockets.remove(data_handle);
                    session.data_handle = None;
                    session.data_connect_notified = false;
                }
            }

            // --- Read control channel ---
            {
                let mut buf = [0u8; 2048];
                loop {
                    let ctrl_socket = sockets.get_mut::<TcpSocket>(session.control_handle);
                    match ctrl_socket.recv_slice(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let action = session.client.receive_control(&buf[..n]);
                            handle_ftp_action(session, sockets, iface, action);
                        }
                    }
                }
            }

            // --- Dispatch next queued command if client is ready and no data transfer is pending ---
            if session.client.is_ready() && session.data_handle.is_none() {
                if let Some(cmd) = session.command_queue.first() {
                    // We need to take the command out, but we borrow session.
                    // Use swap_remove(0) — order doesn't matter for single-consumer queues,
                    // but we want FIFO. Use remove(0) (O(n) but queues are small).
                    let cmd = session.command_queue.remove(0);
                    dispatch_command(session, sockets, iface, cmd);
                }
            }

            // Check if session is fully done.
            if session.client.is_done() {
                serial_println!("[FTP] Session {} done", session.session_id);
                close_session_sockets(session, sockets);
                session.closed = true;
            }
        }

        FtpJobState::Done => {
            // Terminal state — should not be re-inserted, but handle gracefully.
            session.closed = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch a single FtpCommand to the state machine
// ---------------------------------------------------------------------------

fn dispatch_command(
    session: &mut FtpSession,
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    cmd: FtpCommand,
) {
    session.current_callback_id = Some(cmd.callback_id.clone());

    let action = match cmd.kind {
        FtpCommandKind::List(path) => {
            session.client.list(&path)
        }
        FtpCommandKind::Get(remote_path, store_key) => {
            session.current_get_store_key = Some(store_key);
            session.client.get(&remote_path)
        }
        FtpCommandKind::Put(remote_path, store_key) => {
            let data = storage::read_object(&store_key).unwrap_or_default();
            session.client.put(&remote_path, data)
        }
        FtpCommandKind::Mkdir(path) => {
            session.client.mkdir(&path)
        }
        FtpCommandKind::Delete(path) => {
            session.client.delete(&path)
        }
        FtpCommandKind::Close => {
            session.client.quit()
        }
    };

    handle_ftp_action(session, sockets, iface, action);
}

// ---------------------------------------------------------------------------
// Handle a single FtpAction returned by the state machine
// ---------------------------------------------------------------------------

fn handle_ftp_action(
    session: &mut FtpSession,
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    action: FtpAction,
) {
    match action {
        FtpAction::SendControl(data) => {
            let ctrl_socket = sockets.get_mut::<TcpSocket>(session.control_handle);
            if ctrl_socket.can_send() {
                let _ = ctrl_socket.send_slice(&data);
            } else {
                serial_println!("[FTP] Session {} cannot send control data", session.session_id);
            }
        }

        FtpAction::ConnectData(ip_u32, port) => {
            // Reconstruct IPv4 from big-endian u32.
            let a = (ip_u32 >> 24) as u8;
            let b = (ip_u32 >> 16) as u8;
            let c = (ip_u32 >> 8) as u8;
            let d = ip_u32 as u8;
            let data_addr = IpAddress::Ipv4(Ipv4Address::new(a, b, c, d));

            let tcp_rx = tcp::SocketBuffer::new(vec![0u8; 65536]);
            let tcp_tx = tcp::SocketBuffer::new(vec![0u8; 4096]);
            let data_socket = TcpSocket::new(tcp_rx, tcp_tx);
            let data_handle = sockets.add(data_socket);

            let local_port = super::next_local_port();
            let remote_endpoint = IpEndpoint::new(data_addr, port);
            let cx = iface.context();
            let data_socket = sockets.get_mut::<TcpSocket>(data_handle);
            match data_socket.connect(cx, remote_endpoint, local_port) {
                Ok(()) => {
                    serial_println!("[FTP] Session {} data channel connecting to {}:{}", session.session_id, data_addr, port);
                    session.data_handle = Some(data_handle);
                    session.data_connect_notified = false;
                }
                Err(e) => {
                    serial_println!("[FTP] Session {} data connect error: {:?}", session.session_id, e);
                    sockets.remove(data_handle);
                    push_current_error(session, format!("Error: Data Connect Failed: {:?}", e));
                }
            }
        }

        FtpAction::SendData(data) => {
            if let Some(data_handle) = session.data_handle {
                let data_socket = sockets.get_mut::<TcpSocket>(data_handle);
                if data_socket.can_send() {
                    let _ = data_socket.send_slice(&data);
                }
                // Signal end-of-upload by half-closing the send side.
                data_socket.close();
            }
        }

        FtpAction::DataComplete(data) => {
            // If this was a GET, save to JSKV.
            let msg = if let Some(ref key) = session.current_get_store_key.clone() {
                storage::write_object(key, &data);
                serial_println!("[FTP] Session {} GET complete: {} bytes -> store:{}", session.session_id, data.len(), key);
                session.current_get_store_key = None;
                format!("OK:{}", data.len())
            } else {
                // LIST or PUT complete — return the data as a UTF-8 string.
                let s = String::from_utf8_lossy(&data).into_owned();
                serial_println!("[FTP] Session {} DataComplete: {} bytes", session.session_id, data.len());
                s
            };

            if let Some(cb) = session.current_callback_id.take() {
                session.pending_results.push(FtpResultItem {
                    callback_id: cb,
                    success: true,
                    data: msg,
                });
            }
        }

        FtpAction::Ok(msg) => {
            serial_println!("[FTP] Session {} OK: {}", session.session_id, msg);
            if let Some(cb) = session.current_callback_id.take() {
                session.pending_results.push(FtpResultItem {
                    callback_id: cb,
                    success: true,
                    data: msg,
                });
            }
        }

        FtpAction::Error(e) => {
            let msg = format!("Error: {:?}", e);
            serial_println!("[FTP] Session {} Error: {}", session.session_id, msg);
            if let Some(cb) = session.current_callback_id.take() {
                session.pending_results.push(FtpResultItem {
                    callback_id: cb,
                    success: false,
                    data: msg,
                });
            }
        }

        FtpAction::NeedMore => {}
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn push_pending_error(session: &mut FtpSession, msg: String) {
    // Error any in-flight callback.
    if let Some(cb) = session.current_callback_id.take() {
        session.pending_results.push(FtpResultItem {
            callback_id: cb,
            success: false,
            data: msg.clone(),
        });
    }
    // Error all queued commands.
    for cmd in session.command_queue.drain(..) {
        session.pending_results.push(FtpResultItem {
            callback_id: cmd.callback_id,
            success: false,
            data: msg.clone(),
        });
    }
}

fn push_current_error(session: &mut FtpSession, msg: String) {
    if let Some(cb) = session.current_callback_id.take() {
        session.pending_results.push(FtpResultItem {
            callback_id: cb,
            success: false,
            data: msg,
        });
    }
}

/// Flush `pending_results` into FTP_RESULTS for JS delivery.
fn collect_results(session: &FtpSession) {
    if session.pending_results.is_empty() {
        return;
    }
    let mut results = super::FTP_RESULTS.lock();
    for item in &session.pending_results {
        results.push((session.pid, session.session_id, item.success, item.data.clone()));
    }
}

fn close_session_sockets(session: &mut FtpSession, sockets: &mut SocketSet<'static>) {
    // Flush pending results before closing.
    collect_results(session);
    session.pending_results.clear();

    if let Some(data_handle) = session.data_handle.take() {
        let s = sockets.get_mut::<TcpSocket>(data_handle);
        s.close();
        sockets.remove(data_handle);
    }
    let ctrl_socket = sockets.get_mut::<TcpSocket>(session.control_handle);
    ctrl_socket.close();
    sockets.remove(session.control_handle);
    session.job_state = FtpJobState::Done;
}

pub fn cleanup_process_ftp(pid: u32, sockets: &mut SocketSet<'static>) {
    let session_ids: Vec<u64> = {
        let sessions = super::FTP_SESSIONS.lock();
        sessions
            .iter()
            .filter_map(|(id, s)| if s.pid == pid { Some(*id) } else { None })
            .collect()
    };

    for session_id in session_ids {
        let session_opt = super::FTP_SESSIONS.lock().remove(&session_id);
        if let Some(mut session) = session_opt {
            close_session_sockets(&mut session, sockets);
            serial_println!("[FTP] Cleaned up session {} for pid={}", session_id, pid);
        }
    }
}

fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let a = parts[0].parse::<u8>().ok()?;
    let b = parts[1].parse::<u8>().ok()?;
    let c = parts[2].parse::<u8>().ok()?;
    let d = parts[3].parse::<u8>().ok()?;
    Some(Ipv4Address::new(a, b, c, d))
}
