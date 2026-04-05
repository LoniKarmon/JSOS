use crate::println;
use crate::serial_println;
use rtl8139::NIC;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::dhcpv4::{Socket as Dhcpv4Socket};
use smoltcp::socket::dns::{Socket as DnsSocket, GetQueryResultError};
use alloc::vec;
use alloc::boxed::Box;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address, DnsQueryType};

use smoltcp::socket::tcp::{self, Socket as TcpSocket, State};
use smoltcp::time::Instant;
use spin::Mutex;
use lazy_static::lazy_static;
use embedded_tls::{TlsContext, TlsConfig, TlsConnection, Aes128GcmSha256, TlsError, NoVerify};
// use rand_chacha::rand_core::RngCore;
// use futures_util::{FutureExt, task::noop_waker_ref};
use embedded_io_async::{Read, Write, ErrorType};
use core::task::Context;
use core::pin::Pin;
use core::future::Future;
use rand_chacha::rand_core::SeedableRng;
use alloc::string::{ToString, String};
use alloc::vec::Vec;
use alloc::format;

pub mod rtl8139;
pub mod tftp_job;
pub mod ftp_job;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    IoError,
    ConnectionClosed,
}

impl embedded_io_async::Error for NetError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

pub struct NonBlockingSmoltcpIo {
    pub handle: SocketHandle,
}

impl ErrorType for NonBlockingSmoltcpIo {
    type Error = NetError;
}

impl Read for NonBlockingSmoltcpIo {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        use core::task::Poll;
        use futures_util::future::poll_fn;
        poll_fn(|_cx| {
            unsafe {
                if FETCH_SOCKETS_PTR.is_null() { return Poll::Pending; }
                let sockets = &mut *FETCH_SOCKETS_PTR;
                let socket = sockets.get_mut::<TcpSocket>(self.handle);
                if socket.can_recv() {
                    match socket.recv_slice(buf) {
                        Ok(n) => Poll::Ready(Ok(n)),
                        // recv_slice can fail with RecvError::Finished when a FIN arrives
                        // concurrently with data. Treat it as EOF (Ok(0)) rather than a
                        // hard error — embedded-tls will handle it gracefully.
                        Err(_) => Poll::Ready(Ok(0)),
                    }
                } else if !socket.may_recv() {
                    Poll::Ready(Ok(0))
                } else if !socket.is_active() || !socket.is_open() {
                    Poll::Ready(Ok(0))
                } else {
                    Poll::Pending
                }
            }
        }).await
    }
}

impl Write for NonBlockingSmoltcpIo {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, NetError> {
        use core::task::Poll;
        use futures_util::future::poll_fn;
        poll_fn(|_cx| {
            unsafe {
                if FETCH_SOCKETS_PTR.is_null() { return Poll::Pending; }
                let sockets = &mut *FETCH_SOCKETS_PTR;
                let socket = sockets.get_mut::<TcpSocket>(self.handle);
                if socket.can_send() {
                    let res = socket.send_slice(buf);
                    Poll::Ready(res.map_err(|_| NetError::IoError))
                } else if !socket.is_active() || !socket.is_open() {
                    Poll::Ready(Err(NetError::ConnectionClosed))
                } else {
                    Poll::Pending
                }
            }
        }).await
    }
}

pub static mut FETCH_SOCKETS_PTR: *mut SocketSet<'static> = core::ptr::null_mut();

lazy_static! {
    pub static ref IFACE: Mutex<Option<Interface>> = Mutex::new(None);
    pub static ref SOCKETS: Mutex<Option<SocketSet<'static>>> = Mutex::new(None);
    pub static ref FETCH_JOBS: Mutex<alloc::vec::Vec<Box<FetchJob>>> = Mutex::new(alloc::vec::Vec::new());
    pub static ref WEBSOCKET_JOBS: Mutex<alloc::collections::BTreeMap<u32, Box<WebSocketJob>>> = Mutex::new(alloc::collections::BTreeMap::new());
    pub static ref NEXT_WS_HANDLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);
    pub static ref SERVER_JOBS: Mutex<alloc::vec::Vec<Box<ServerJob>>> = Mutex::new(alloc::vec::Vec::new());
    pub static ref LISTENERS: Mutex<alloc::vec::Vec<ListeningSocket>> = Mutex::new(alloc::vec::Vec::new());
    pub static ref CUSTOM_HTTP_RESPONSE: Mutex<Option<alloc::string::String>> = Mutex::new(None);
    pub static ref TFTP_JOBS: Mutex<alloc::vec::Vec<Box<tftp_job::TftpJob>>> = Mutex::new(alloc::vec::Vec::new());
    pub static ref TFTP_RESULTS: Mutex<alloc::vec::Vec<(u32, alloc::string::String, alloc::string::String)>> = Mutex::new(alloc::vec::Vec::new());
    pub static ref FTP_SESSIONS: Mutex<alloc::collections::BTreeMap<u64, Box<ftp_job::FtpSession>>> = Mutex::new(alloc::collections::BTreeMap::new());
    /// (pid, session_id, success, data)
    pub static ref FTP_RESULTS: Mutex<alloc::vec::Vec<(u32, u64, bool, alloc::string::String)>> = Mutex::new(alloc::vec::Vec::new());
}

pub fn set_http_response(html: alloc::string::String) {
    *CUSTOM_HTTP_RESPONSE.lock() = Some(html);
}

pub static NEXT_EPHEMERAL_PORT: core::sync::atomic::AtomicU16 =
    core::sync::atomic::AtomicU16::new(49152);

pub struct TlsHandshakeContext {
    pub config: TlsConfig<'static, Aes128GcmSha256>,
    pub rng: rand_chacha::ChaCha20Rng,
}

// ── Per-job fetch state ────────────────────────────────────────────────────

pub struct FetchJob {
    pub pid: u32,
    pub url: alloc::string::String,
    pub host: alloc::string::String,
    pub path: alloc::string::String,
    pub method: alloc::string::String,
    pub body: alloc::string::String,
    pub headers_json: alloc::string::String,
    pub port: u16,
    pub is_https: bool,
    /// State machine:
    ///   0 = wait for DHCP
    ///   1 = start DNS query
    ///   2 = wait for DNS result
    ///   3 = connect TCP
    ///   4 = wait for Established
    ///   5 = TLS Handshake
    ///   6 = TLS Send Request
    ///   7 = TLS Receive Response
    ///   8 = Plain HTTP Send Request
    ///   9 = Plain HTTP Receive Response
    pub state: u8,
    pub remote_addr: Option<smoltcp::wire::IpAddress>,
    pub response: alloc::string::String,
    pub dns_query_handle: Option<smoltcp::socket::dns::QueryHandle>,
    pub tcp_handle: Option<SocketHandle>,
    pub tls_handshake_done: bool,
    pub tls_read_buffer: alloc::vec::Vec<u8>,
    pub tls_write_buffer: alloc::vec::Vec<u8>,
    pub start_ticks: u64,
    pub last_nodata_ticks: u64,
    pub redirect_count: u8,
    // We store the TlsConnection as a usize to keep it alive across ticks
    // safely (Send-wise). We cast it to a raw pointer when needed.
    pub tls_connection_ptr_val: usize, 
    pub tls_handshake_future_ptr_val: usize,
    pub tls_read_future_ptr_val: usize,
    pub tls_write_future_ptr_val: usize,
    pub tls_handshake_context_ptr_val: usize,
    pub tls_send_data: alloc::vec::Vec<u8>,
    /// Optional ALPN protocol list override. If None, defaults to ["http/1.1"] for HTTPS.
    pub alpn_protocols: Option<alloc::vec::Vec<alloc::string::String>>,
}

pub struct WebSocketJob {
    pub pid: u32,
    pub url: alloc::string::String,
    pub host: alloc::string::String,
    pub path: alloc::string::String,
    pub port: u16,
    pub is_https: bool,
    pub state: u8,
    pub remote_addr: Option<smoltcp::wire::IpAddress>,
    pub dns_query_handle: Option<smoltcp::socket::dns::QueryHandle>,
    pub tcp_handle: Option<SocketHandle>,
    pub start_ticks: u64,
    pub last_activity_ticks: u64,

    // TLS fields (mirrors FetchJob)
    pub tls_connection_ptr_val: usize,
    pub tls_handshake_future_ptr_val: usize,
    pub tls_read_future_ptr_val: usize,
    pub tls_write_future_ptr_val: usize,
    pub tls_handshake_context_ptr_val: usize,
    pub tls_send_data: alloc::vec::Vec<u8>,
    pub tls_read_buffer: alloc::vec::Vec<u8>,
    pub tls_write_buffer: alloc::vec::Vec<u8>,

    // WebSocket specific
    pub sec_key: alloc::string::String,
    pub rx_queue: alloc::vec::Vec<alloc::string::String>,
    pub tx_queue: alloc::vec::Vec<alloc::string::String>,
    pub frame_buffer: alloc::vec::Vec<u8>,
    pub closing: bool,
    pub closed: bool,
    /// Optional ALPN protocol list override. If None, defaults to ["http/1.1"] for WSS.
    pub alpn_protocols: Option<alloc::vec::Vec<alloc::string::String>>,
}

pub struct ListeningSocket {
    pub pid: u32,
    pub port: u16,
    pub handle: SocketHandle,
}

pub struct ServerJob {
    pub pid: u32,
    pub handle: SocketHandle,
    pub buffer: alloc::vec::Vec<u8>,
    pub response: Option<alloc::vec::Vec<u8>>,
    pub bytes_sent: usize,
    pub start_ticks: u64,
}



fn get_timestamp() -> Instant {
    let ticks = crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let hz = crate::interrupts::TICKS_PER_SEC.load(core::sync::atomic::Ordering::Relaxed).max(1);
    Instant::from_millis((ticks * 1000 / hz) as i64)
}

fn next_local_port() -> u16 {
    let p = NEXT_EPHEMERAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if p > 65000 {
        NEXT_EPHEMERAL_PORT.store(49152, core::sync::atomic::Ordering::Relaxed);
    }
    p
}

// -- Helpers for WebSocket Handshake ────────────────────────────────────────

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let mut padded = data.to_vec();
    let orig_len_bits = (data.len() as u64) * 8;
    padded.push(0x80);
    while (padded.len() * 8) % 512 != 448 {
        padded.push(0);
    }
    padded.extend_from_slice(&orig_len_bits.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..80 {
            w[i] = (w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A827999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1BBCDC)
            } else {
                (b ^ c ^ d, 0xCA62C1D6)
            };

            let temp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i];
        let b1 = if i + 1 < data.len() { Some(data[i+1]) } else { None };
        let b2 = if i + 2 < data.len() { Some(data[i+2]) } else { None };

        out.push(CHARSET[(b0 >> 2) as usize] as char);
        out.push(CHARSET[(((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4)) as usize] as char);
        
        if let Some(b1_val) = b1 {
            out.push(CHARSET[(((b1_val & 0x0F) << 2) | (b2.unwrap_or(0) >> 6)) as usize] as char);
        } else {
            out.push('=');
        }

        if let Some(b2_val) = b2 {
            out.push(CHARSET[(b2_val & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

pub fn base64_decode(input: &str) -> Vec<u8> {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    
    for &b in input.as_bytes() {
        if b == b'=' { break; }
        let val = match CHARSET.iter().position(|&c| c == b) {
            Some(v) => v as u32,
            None => continue,
        };
        buffer = (buffer << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

// ── Public API ─────────────────────────────────────────────────────────────

pub fn get_net_info() -> alloc::string::String {
    let mut info = alloc::string::String::new();
    if let Some(ref nic) = *NIC.lock() {
        let mac = EthernetAddress::from_bytes(&nic.mac_address());
        info.push_str(&alloc::format!("MAC: {}\n", mac));
    }
    if let Some(ref iface) = *IFACE.lock() {
        for addr in iface.ip_addrs() {
            info.push_str(&alloc::format!("IP: {}\n", addr));
        }
        // Routes iteration is version-dependent in smoltcp, skipping for now
        /*
        for (dest, route) in iface.routes().iter() {
           info.push_str(&alloc::format!("Route: {} -> {}\n", dest, route.via_router));
        }
        */
    }
    info
}

pub fn start_listen(pid: u32, port: u16) {
    let mut socket = TcpSocket::new(
        tcp::SocketBuffer::new(vec![0; 32768]),
        tcp::SocketBuffer::new(vec![0; 32768])
    );
    
    match socket.listen(port) {
        Ok(_) => {
            let mut sockets_guard = SOCKETS.lock();
            if let Some(ref mut sockets) = *sockets_guard {
                let handle = sockets.add(socket);
                LISTENERS.lock().push(ListeningSocket { pid, port, handle });
                serial_println!("[NET] PID {} listening on port {}", pid, port);
            }
        }
        Err(e) => {
            serial_println!("[NET] Failed to listen on port {}: {:?}", port, e);
        }
    }
}

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

/// Open an FTP control connection. Returns session_id (>= 0) or -1 on error.
pub fn ftp_connect(pid: u32, server_ip: &str, user: &str, pass: &str) -> i64 {
    let mut sockets_guard = SOCKETS.lock();
    if let Some(ref mut sockets) = *sockets_guard {
        ftp_job::create_ftp_session(pid, server_ip, user, pass, sockets)
    } else {
        -1
    }
}

/// Enqueue an FTP command on an existing session.
pub fn ftp_command(session_id: u64, cmd: ftp_job::FtpCommand) {
    ftp_job::enqueue_ftp_command(session_id, cmd);
}

pub fn start_fetch(pid: u32, url: &str, method: &str, body: &str, headers_json: &str, alpn_protocols: Option<alloc::vec::Vec<alloc::string::String>>) {
    let is_https = url.starts_with("https://");
    let url_body = if is_https {
        url.trim_start_matches("https://")
    } else {
        url.trim_start_matches("http://")
    };

    let slash_idx = url_body.find('/').unwrap_or(url_body.len());
    let host_part = &url_body[..slash_idx];
    let path = if slash_idx < url_body.len() {
        alloc::string::String::from(&url_body[slash_idx..])
    } else {
        alloc::string::String::from("/")
    };

    // Parse port from host if present (e.g. "localhost:1234")
    let (host, port) = if let Some(colon_idx) = host_part.find(':') {
        let h = alloc::string::String::from(&host_part[..colon_idx]);
        let p_str = &host_part[colon_idx + 1..];
        let p = p_str.parse::<u16>().unwrap_or(if is_https { 443 } else { 80 });
        (h, p)
    } else {
        (alloc::string::String::from(host_part), if is_https { 443 } else { 80 })
    };

    let job = FetchJob {
        pid,
        url: alloc::string::String::from(url),
        host,
        path,
        method: alloc::string::String::from(method),
        body: alloc::string::String::from(body),
        headers_json: alloc::string::String::from(headers_json),
        port,
        is_https,
        state: 0,
        remote_addr: None,
        response: alloc::string::String::new(),
        dns_query_handle: None,
        tcp_handle: None,
        tls_handshake_done: false,
        tls_read_buffer: vec![0u8; 16384],
        tls_write_buffer: vec![0u8; 16384],
        start_ticks: crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed),
        last_nodata_ticks: 0,
        redirect_count: 0,
        tls_connection_ptr_val: 0,
        tls_handshake_future_ptr_val: 0,
        tls_read_future_ptr_val: 0,
        tls_write_future_ptr_val: 0,
        tls_handshake_context_ptr_val: 0,
        tls_send_data: alloc::vec::Vec::new(),
        alpn_protocols,
    };

    FETCH_JOBS.lock().push(Box::new(job));
}

pub fn start_websocket(pid: u32, url: &str, alpn_protocols: Option<alloc::vec::Vec<alloc::string::String>>) -> i32 {
    let (_is_https, host, port, path) = parse_url(url);
    if url.starts_with("ws://") == false && url.starts_with("wss://") == false {
        return -1;
    }
    let is_https = url.starts_with("wss://");

    // Generate a random 16-byte key for Sec-WebSocket-Key
    let mut key_bytes = [0u8; 16];
    let ticks = crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(ticks + pid as u64);
    use rand_chacha::rand_core::RngCore;
    rng.fill_bytes(&mut key_bytes);
    let sec_key = base64_encode(&key_bytes);

    let job = WebSocketJob {
        pid,
        url: url.into(),
        host,
        path,
        port,
        is_https,
        state: 0,
        remote_addr: None,
        dns_query_handle: None,
        tcp_handle: None,
        start_ticks: ticks,
        last_activity_ticks: ticks,
        tls_connection_ptr_val: 0,
        tls_handshake_future_ptr_val: 0,
        tls_read_future_ptr_val: 0,
        tls_write_future_ptr_val: 0,
        tls_handshake_context_ptr_val: 0,
        tls_send_data: Vec::new(),
        tls_read_buffer: vec![0u8; 16384],
        tls_write_buffer: vec![0u8; 16384],
        sec_key: sec_key.clone(),
        rx_queue: Vec::new(),
        tx_queue: Vec::new(),
        frame_buffer: Vec::new(),
        closing: false,
        closed: false,
        alpn_protocols,
    };

    let handle = NEXT_WS_HANDLE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    WEBSOCKET_JOBS.lock().insert(handle, Box::new(job));
    handle as i32
}

pub fn init() {
    rtl8139::init();

    let mut nic_guard = NIC.lock();
    if let Some(ref mut nic) = *nic_guard {
        let mac_addr = nic.mac_address();
        let hw_addr = HardwareAddress::Ethernet(EthernetAddress(mac_addr));

        let mut config = Config::new(hw_addr);
        config.random_seed = 0x12345678;

        let iface = Interface::new(config, &mut *nic, get_timestamp());
        let mut sockets = SocketSet::new(vec![]);

        // DHCP
        sockets.add(Dhcpv4Socket::new());

        // DNS (Google Public DNS)
        let dns_servers = [Ipv4Address::new(8, 8, 8, 8).into()];
        sockets.add(DnsSocket::new(&dns_servers, vec![]));

        // NOTE: We no longer pre-allocate a global TCP socket.
        // Each FetchJob allocates its own socket when it starts connecting
        // (State 3) so sockets can never conflict.

        *IFACE.lock() = Some(iface);
        *SOCKETS.lock() = Some(sockets);

        serial_println!("[INFO] Network stack initialized. DHCP Client starting...");
    } else {
        println!("No NIC found. Network stack disabled.");
    }
}


pub fn poll_network() {
    let mut iface_locked = IFACE.lock();
    let mut sockets_locked = SOCKETS.lock();
    let mut jobs_locked = FETCH_JOBS.lock();

    if iface_locked.is_none() || sockets_locked.is_none() {
        return;
    }

    let iface = iface_locked.as_mut().unwrap();
    let sockets = sockets_locked.as_mut().unwrap();
    let jobs = &mut *jobs_locked;

    // Set global pointer for non-blocking IO to find sockets
    unsafe {
        FETCH_SOCKETS_PTR = sockets as *mut SocketSet;
    }

    let current_ticks = crate::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let mut finished_jobs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();

    // Drive the network stack forward before inspecting socket state
    let mut nic_guard = NIC.lock();
    if nic_guard.is_none() {
        return;
    }
    let nic = nic_guard.as_mut().unwrap();
    iface.poll(get_timestamp(), nic, sockets);

    // Drive TFTP jobs
    tftp_job::poll_tftp_jobs(sockets, current_ticks);

    // Drive FTP sessions
    ftp_job::poll_ftp_sessions(sockets, iface, current_ticks);

    // Drive TCP servers
    poll_server_sockets(sockets, current_ticks);

    // Drive WebSockets
    poll_websocket_jobs(sockets, iface, current_ticks);



    // Advance DHCP while we're in here
    for (_handle, socket) in sockets.iter_mut() {
        if let smoltcp::socket::Socket::Dhcpv4(dhcp) = socket {
            if let Some(smoltcp::socket::dhcpv4::Event::Configured(config)) = dhcp.poll() {
                iface.update_ip_addrs(|addrs| { let _ = addrs.push(IpCidr::Ipv4(config.address)); });
                if let Some(router) = config.router {
                    iface.routes_mut().add_default_ipv4_route(router).unwrap();
                }
            }
        }
    }


    let n_jobs = jobs.len();
    for idx in 0..n_jobs {
        // Use raw pointer to completely decouple from 'jobs' lifetime
        // Dereference the Box to get to the FetchJob itself
        let job_ptr = &mut *jobs[idx] as *mut FetchJob;
        let job = unsafe { &mut *job_ptr };
        // Long timeout (approx 30 s) so slow real-world hosts don't get cut off
        if current_ticks.saturating_sub(job.start_ticks) > 3000 {
            serial_println!("[FETCH] Timeout for {} (state={})", job.url, job.state);
            job.response = alloc::string::String::from("Error: Network Timeout.");
            // Clean up the per-job TCP socket if it was opened
            if let Some(h) = job.tcp_handle.take() {
                sockets.remove(h);
            }
            finished_jobs.push(idx);
            continue;
        }

        match job.state {
            // -- 0: wait until DHCP gives us an IP --------------------------
            0 => {
                if iface.ipv4_addr().is_some() {
                    job.state = 1;
                }
            }

            // -- 1: kick off DNS query -------------------------------------
            1 => {
                let dns_handle = sockets
                    .iter()
                    .find_map(|(h, s)| if matches!(s, smoltcp::socket::Socket::Dns(_)) { Some(h) } else { None })
                    .unwrap();
                let dns_socket = sockets.get_mut::<DnsSocket>(dns_handle);

                match dns_socket.start_query(iface.context(), &job.host, DnsQueryType::A) {
                    Ok(qh) => {
                        serial_println!("[FETCH] DNS query started for {}", job.host);
                        job.dns_query_handle = Some(qh);
                        job.state = 2;
                    }
                    Err(_) => {
                        serial_println!("[FETCH] DNS start failed for {}", job.host);
                        job.response = alloc::string::String::from("Error: DNS Query Failed.");
                        finished_jobs.push(idx);
                    }
                }
            }

            // -- 2: wait for DNS answer ------------------------------------
            2 => {
                let dns_handle = sockets
                    .iter()
                    .find_map(|(h, s)| if matches!(s, smoltcp::socket::Socket::Dns(_)) { Some(h) } else { None })
                    .unwrap();
                let dns_socket = sockets.get_mut::<DnsSocket>(dns_handle);
                let qh = job.dns_query_handle.unwrap();

                match dns_socket.get_query_result(qh) {
                    Ok(addrs) => {
                        if let Some(addr) = addrs.first() {
                            serial_println!("[FETCH] DNS resolved {} -> {}", job.host, addr);
                            job.remote_addr = Some(*addr);
                            job.state = 3;
                        }
                    }
                    Err(GetQueryResultError::Pending) => {}
                    Err(_) => {
                        serial_println!("[FETCH] DNS failed for {}", job.host);
                        job.response = alloc::string::String::from("Error: DNS Resolution Failed.");
                        finished_jobs.push(idx);
                    }
                }
            }

            // -- 3: allocate a FRESH TCP socket and initiate connect --------
            3 => {
                // Allocate a large RX buffer for certificate chains
                let tcp_rx = tcp::SocketBuffer::new(vec![0u8; 32768]);
                let tcp_tx = tcp::SocketBuffer::new(vec![0u8; 16384]);
                let tcp_socket = TcpSocket::new(tcp_rx, tcp_tx);
                let handle = sockets.add(tcp_socket);
                job.tcp_handle = Some(handle);

                let cx = iface.context();
                let port = job.port;
                let remote_endpoint = (job.remote_addr.unwrap(), port);
                let local_port = next_local_port();

                serial_println!("[FETCH] Connecting to {}:{} via local:{}", job.remote_addr.unwrap(), port, local_port);

                let tcp_socket = sockets.get_mut::<TcpSocket>(handle);
                match tcp_socket.connect(cx, remote_endpoint, local_port) {
                    Ok(()) => { job.state = 4; }
                    Err(e) => {
                        serial_println!("[FETCH] Connect error: {:?}", e);
                        sockets.remove(handle);
                        job.tcp_handle = None;
                        job.response = alloc::string::String::from("Error: TCP Connect Failed.");
                        finished_jobs.push(idx);
                    }
                }
            }

            // -- 4: wait for Established, then transition to Handshake or IO --
            4 => {
                let handle = job.tcp_handle.unwrap();
                let tcp_socket = sockets.get_mut::<TcpSocket>(handle);

                match tcp_socket.state() {
                    State::Established => {
                        serial_println!("[FETCH] TCP Established for {}", job.host);
                        if job.is_https {
                            job.state = 5; // Handshake
                        } else {
                            job.state = 8; // HTTP Write
                        }
                        job.start_ticks = current_ticks;
                    }
                    // Connection was refused / reset before ESTABLISHED
                    State::Closed | State::CloseWait => {
                        serial_println!("[FETCH] Connection refused for {}", job.url);
                        sockets.remove(handle);
                        job.tcp_handle = None;
                        job.response = alloc::string::String::from("Error: TCP Connection Refused.");
                        finished_jobs.push(idx);
                    }
                    _ => {} // SynSent / SynReceived - still waiting
                }
            }

            // -- 5: TLS Handshake (Non-blocking) --
            5 => {
                let handle = job.tcp_handle.unwrap();
                let handshake_result = {
                    use futures_util::task::noop_waker_ref;
                    
                    // 1. Initialize Connection
                    if job.tls_connection_ptr_val == 0 {
                        serial_println!("[FETCH] Initializing TLS session for {}", job.host);
                        let read_buf = Box::new([0u8; 32768]);
                        let write_buf = Box::new([0u8; 32768]);
                        let io = NonBlockingSmoltcpIo { handle };
                        let connection: TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256> = TlsConnection::new(io, Box::leak(read_buf), Box::leak(write_buf));
                        job.tls_connection_ptr_val = Box::into_raw(Box::new(connection)) as usize;
                    }

                    // 2. Initialize Future and Context
                    if job.tls_handshake_future_ptr_val == 0 {
                        let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                        let rng = rand_chacha::ChaCha20Rng::seed_from_u64(current_ticks as u64 + idx as u64);
                        
                        // Leak hostname for 'static TlsConfig
                        let host_leaked = Box::leak(job.host.clone().into_boxed_str());
                        let config = if let Some(ref custom) = job.alpn_protocols {
                            let leaked: alloc::vec::Vec<&'static [u8]> = custom.iter()
                                .map(|s| Box::leak(s.clone().into_bytes().into_boxed_slice()) as &'static [u8])
                                .collect();
                            let leaked_slice: &'static [&'static [u8]] = Box::leak(leaked.into_boxed_slice());
                            TlsConfig::new().with_server_name(host_leaked).with_alpn_protocols(leaked_slice)
                        } else {
                            TlsConfig::new().with_server_name(host_leaked).with_alpn_protocols(&[b"http/1.1"])
                        };

                        let context = Box::leak(Box::new(TlsHandshakeContext { config, rng }));
                        job.tls_handshake_context_ptr_val = context as *mut _ as usize;

                        // Create the future and erase its type to store it
                        let future = connection.open::<_, NoVerify>(TlsContext::new(&context.config, &mut context.rng));
                        let boxed_future: Pin<Box<dyn Future<Output = Result<(), TlsError>> + 'static>> = Box::pin(future);
                        job.tls_handshake_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                    }

                    // 3. Poll the existing future
                    let future_ptr = job.tls_handshake_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>>>>;
                    let future = unsafe { &mut *future_ptr };
                    
                    let res = future.as_mut().poll(&mut Context::from_waker(noop_waker_ref()));
                    res
                };

                match handshake_result {
                    core::task::Poll::Ready(Ok(_)) => {
                        serial_println!("[FETCH] TLS Handshake complete for {}", job.host);
                        job.state = 6; // next: send request
                        job.start_ticks = current_ticks;
                    }
                    core::task::Poll::Ready(Err(e)) => {
                        serial_println!("[FETCH] TLS Handshake error: {:?}", e);
                        job.response = alloc::format!("Error: TLS Handshake Failed: {:?}", e);
                        cleanup_job(job, sockets, &mut finished_jobs, idx);
                    }
                    core::task::Poll::Pending => {
                        if current_ticks.saturating_sub(job.start_ticks) > 400 {
                            serial_println!("[FETCH] TLS Handshake timeout");
                            job.response = alloc::string::String::from("Error: TLS Handshake Timeout.");
                            cleanup_job(job, sockets, &mut finished_jobs, idx);
                        }
                    }
                }
            }

            // -- 6: TLS Send Request (Non-blocking) --
            6 => {
                if job.tls_write_future_ptr_val == 0 {
                    let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                    let mut request = alloc::format!("{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", job.method, job.path, job.host);
                    for (k, v) in parse_headers_simple(&job.headers_json) {
                        request.push_str(&alloc::format!("{}: {}\r\n", k, v));
                    }
                    if !job.body.is_empty() {
                        request.push_str(&alloc::format!("Content-Length: {}\r\n", job.body.len()));
                    }
                    request.push_str("\r\n");
                    request.push_str(&job.body);
                    
                    job.tls_send_data = request.into_bytes();
                    
                    // The future borrows from job.tls_send_data. Since FetchJob is Boxed, the address of tls_send_data's heap storage is stable...
                    // WAIT! Vec's internal pointer might move if we push to IT, but we don't push to it once established.
                    // BUT, to be absolutely safe, let's leak the data.
                    let data_slice: &'static [u8] = unsafe { core::slice::from_raw_parts(job.tls_send_data.as_ptr(), job.tls_send_data.len()) };
                    
                    let future = connection.write(data_slice);
                    let boxed_future: Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send + 'static>> = Box::pin(future);
                    job.tls_write_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                }

                let future_ptr = job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>>>>;
                let future = unsafe { &mut *future_ptr };
                
                match future.as_mut().poll(&mut core::task::Context::from_waker(futures_util::task::noop_waker_ref())) {
                    core::task::Poll::Ready(Ok(n)) => {
                        unsafe { let _ = Box::from_raw(future_ptr); }
                        job.tls_write_future_ptr_val = 0;
                        serial_println!("[FETCH] TLS Request buffered ({} bytes). Flushing...", n);
                        job.state = 10; // TLS Flush
                        job.start_ticks = current_ticks;
                    }
                    core::task::Poll::Ready(Err(e)) => {
                        unsafe { let _ = Box::from_raw(future_ptr); }
                        job.tls_write_future_ptr_val = 0;
                        serial_println!("[FETCH] TLS Write error: {:?}", e);
                        job.response = alloc::format!("Error: TLS Write Failed: {:?}", e);
                        cleanup_job(job, sockets, &mut finished_jobs, idx);
                    }
                    core::task::Poll::Pending => {
                        if current_ticks.saturating_sub(job.start_ticks) > 500 {
                            job.response = alloc::string::String::from("Error: TLS Write Timeout.");
                            cleanup_job(job, sockets, &mut finished_jobs, idx);
                        }
                    }
                }
            }

            // -- 10: TLS Flush (Ensure request is sent) --
            10 => {
                if job.tls_write_future_ptr_val == 0 {
                    let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                    let future = connection.flush();
                    let boxed_future: Pin<Box<dyn Future<Output = Result<(), TlsError>> + Send + 'static>> = Box::pin(future);
                    job.tls_write_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                }

                let future_ptr = job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>>>>;
                let future = unsafe { &mut *future_ptr };
                
                match future.as_mut().poll(&mut core::task::Context::from_waker(futures_util::task::noop_waker_ref())) {
                    core::task::Poll::Ready(Ok(())) => {
                        unsafe { let _ = Box::from_raw(future_ptr); }
                        job.tls_write_future_ptr_val = 0;
                        serial_println!("[FETCH] TLS Request flushed and sent.");
                        job.state = 7; // TLS Receive Response
                        job.start_ticks = current_ticks;
                    }
                    core::task::Poll::Ready(Err(e)) => {
                        unsafe { let _ = Box::from_raw(future_ptr); }
                        job.tls_write_future_ptr_val = 0;
                        serial_println!("[FETCH] TLS Flush error: {:?}", e);
                        job.response = alloc::format!("Error: TLS Flush Failed: {:?}", e);
                        cleanup_job(job, sockets, &mut finished_jobs, idx);
                    }
                    core::task::Poll::Pending => {
                        if current_ticks.saturating_sub(job.start_ticks) > 500 {
                            job.response = alloc::string::String::from("Error: TLS Flush Timeout.");
                            cleanup_job(job, sockets, &mut finished_jobs, idx);
                        }
                    }
                }
            }

            // -- 7: TLS Receive Response (Non-blocking + Redirect) --
            7 => {
                loop {
                    if job.tls_read_future_ptr_val == 0 {
                        let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                        // Using job.tls_read_buffer which is stable because FetchJob is Boxed
                        let buffer_slice: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(job.tls_read_buffer.as_mut_ptr(), job.tls_read_buffer.len()) };
                        let future = connection.read(buffer_slice);
                        let boxed_future: Pin<Box<dyn Future<Output = Result<usize, TlsError>> + 'static>> = Box::pin(future);
                        job.tls_read_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                    }

                    let future_ptr = job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>>>>;
                    let future = unsafe { &mut *future_ptr };
                    
                    let poll_res = future.as_mut().poll(&mut core::task::Context::from_waker(futures_util::task::noop_waker_ref()));
                    match poll_res {
                        core::task::Poll::Ready(Ok(0)) => {
                            serial_println!("[FETCH] TLS Decrypted 0 bytes (EOF) from {}", job.host);
                            unsafe { let _ = Box::from_raw(future_ptr); }
                            job.tls_read_future_ptr_val = 0;
                            handle_redirect_or_finish(idx, job, sockets, &mut finished_jobs);
                            break;
                        }
                        core::task::Poll::Ready(Ok(n)) => {
                            unsafe { let _ = Box::from_raw(future_ptr); }
                            job.tls_read_future_ptr_val = 0;
                            
                            if n > 0 {
                                if let Ok(s) = core::str::from_utf8(&job.tls_read_buffer[..n]) {
                                    job.response.push_str(s);
                                }
                                job.start_ticks = current_ticks; 
                                continue; 
                            } else {
                                handle_redirect_or_finish(idx, job, sockets, &mut finished_jobs);
                                break;
                            }
                        }
                        core::task::Poll::Ready(Err(e)) => {
                            unsafe { let _ = Box::from_raw(future_ptr); }
                            job.tls_read_future_ptr_val = 0;

                            // TLS 1.3 servers (e.g. Google) send a NewSessionTicket post-handshake
                            // message before the HTTP response. embedded-tls 0.17 returns IoError
                            // when it encounters this unexpected record type. We retry up to 4 times
                            // to skip past any post-handshake bookkeeping records.
                            // If we already have response data, treat any error as a clean EOF.
                            if !job.response.is_empty() {
                                handle_redirect_or_finish(idx, job, sockets, &mut finished_jobs);
                                break;
                            }
                            
                            // Count retries via last_nodata_ticks (reuse as a counter here)
                            job.last_nodata_ticks += 1;
                            if job.last_nodata_ticks > 4 {
                                serial_println!("[FETCH] TLS Read failed after retries ({}): {:?}", job.host, e);
                                job.response = alloc::format!("Error: TLS Read Failed: {:?}", e);
                                cleanup_job(job, sockets, &mut finished_jobs, idx);
                                break;
                            }
                            serial_println!("[FETCH] TLS record skipped ({}), retry {}/4: {:?}", job.host, job.last_nodata_ticks, e);
                            // Fall through to Pending — try reading the next record next tick
                            break;
                        }
                        core::task::Poll::Pending => {
                            break; // Done for this tick
                        }
                    }
                }
            }

            // -- 8: Plain HTTP Send Request --
            8 => {
                let handle = job.tcp_handle.unwrap();
                let socket = sockets.get_mut::<TcpSocket>(handle);
                if socket.can_send() {
                    let mut request = alloc::format!("{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", job.method, job.path, job.host);
                    
                    // Add custom headers
                    for (k, v) in parse_headers_simple(&job.headers_json) {
                        request.push_str(&alloc::format!("{}: {}\r\n", k, v));
                    }

                    if !job.body.is_empty() {
                        request.push_str(&alloc::format!("Content-Length: {}\r\n", job.body.len()));
                    }
                    request.push_str("\r\n");
                    request.push_str(&job.body);

                    let _ = socket.send_slice(request.as_bytes());
                    job.state = 9; // next: plain receive
                    job.start_ticks = current_ticks;
                } else if current_ticks.saturating_sub(job.start_ticks) > 300 {
                   job.response = "Error: TCP Send Timeout".into();
                   cleanup_job(job, sockets, &mut finished_jobs, idx);
                }
            }

            // -- 9: Plain HTTP Receive Response (Non-blocking + Redirect) --
            9 => {
                let handle = job.tcp_handle.unwrap();
                let tcp_socket = sockets.get_mut::<TcpSocket>(handle);
                if tcp_socket.can_recv() {
                    let mut buf = [0u8; 1024];
                    if let Ok(n) = tcp_socket.recv_slice(&mut buf) {
                        if n > 0 {
                            if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                                job.response.push_str(s);
                            }
                            job.start_ticks = current_ticks;
                        } else {
                            handle_redirect_or_finish(idx, job, sockets, &mut finished_jobs);
                        }
                    }
                } else if !tcp_socket.is_active() || !tcp_socket.is_open() {
                    handle_redirect_or_finish(idx, job, sockets, &mut finished_jobs);
                } else if current_ticks.saturating_sub(job.start_ticks) > 500 {
                    handle_redirect_or_finish(idx, job, sockets, &mut finished_jobs);
                }
            }

            _ => {}
        }
    }

    // Clear global pointer after loop
    unsafe {
        FETCH_SOCKETS_PTR = core::ptr::null_mut();
    }

    // Deliver results in reverse order so removal indices stay valid
    for idx in finished_jobs.into_iter().rev() {
        let job = jobs.remove(idx);

        let sandbox_arc = {
            let list = crate::process::PROCESS_LIST.lock();
            list.get(&job.pid).map(|p| p.sandbox.clone())
        };

        if let Some(sandbox_arc) = sandbox_arc {
            let status_code: u16 = job.response
                .split_once('\n')
                .and_then(|(first_line, _)| {
                    first_line.split_whitespace().nth(1)
                })
                .and_then(|s| s.parse().ok())
                .unwrap_or(200);

            let body_str = if let Some(pos) = job.response.find("\r\n\r\n") {
                &job.response[pos + 4..]
            } else {
                &job.response
            };

            let escaped = body_str
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace('\r', "");

            let script = alloc::format!(
                "if (typeof globalThis.__onFetchResponse === 'function') \
                 {{ globalThis.__onFetchResponse('{}', {}, `{}`); }}",
                job.url, status_code, escaped
            );
            let _ = sandbox_arc.lock().eval(&script);
        }
    }

    // Deliver TFTP results to JS callbacks
    let tftp_results: alloc::vec::Vec<(u32, alloc::string::String, alloc::string::String)> = {
        let mut results = TFTP_RESULTS.lock();
        core::mem::take(&mut *results)
    };
    for (pid, store_key, result) in tftp_results {
        let sandbox_arc = {
            let list = crate::process::PROCESS_LIST.lock();
            list.get(&pid).map(|p| p.sandbox.clone())
        };
        if let Some(sandbox_arc) = sandbox_arc {
            let escaped_key = store_key.replace('\\', "\\\\").replace('\'', "\\'");
            let escaped_result = result.replace('\\', "\\\\").replace('\'', "\\'");
            let script = alloc::format!(
                "if (typeof globalThis.__onTftpResult === 'function') \
                 {{ globalThis.__onTftpResult('{}', '{}'); }}",
                escaped_key, escaped_result
            );
            let _ = sandbox_arc.lock().eval(&script);
        }
    }

    // Deliver FTP results to JS callbacks
    let ftp_results: alloc::vec::Vec<(u32, u64, bool, alloc::string::String)> = {
        let mut results = FTP_RESULTS.lock();
        core::mem::take(&mut *results)
    };
    for (pid, session_id, success, data) in ftp_results {
        let sandbox_arc = {
            let list = crate::process::PROCESS_LIST.lock();
            list.get(&pid).map(|p| p.sandbox.clone())
        };
        if let Some(sandbox_arc) = sandbox_arc {
            let escaped_data = data.replace('\\', "\\\\").replace('\'', "\\'");
            let script = alloc::format!(
                "if (typeof globalThis.__onFtpResult === 'function') \
                 {{ globalThis.__onFtpResult({}, {}, '{}'); }}",
                session_id, if success { "true" } else { "false" }, escaped_data
            );
            let _ = sandbox_arc.lock().eval(&script);
        }
    }
}

fn cleanup_job(job: &mut FetchJob, sockets: &mut SocketSet, finished_jobs: &mut alloc::vec::Vec<usize>, idx: usize) {
    if let Some(handle) = job.tcp_handle {
        let socket = sockets.get_mut::<TcpSocket>(handle);
        socket.abort();
        sockets.remove(handle);
        job.tcp_handle = None;
    }
    if job.tls_connection_ptr_val != 0 {
        unsafe {
             let _ = Box::from_raw(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>);
             job.tls_connection_ptr_val = 0;
        }
    }
    if job.tls_handshake_future_ptr_val != 0 {
        unsafe {
            let _ = Box::from_raw(job.tls_handshake_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>> + Send>>);
            job.tls_handshake_future_ptr_val = 0;
        }
    }
    if job.tls_write_future_ptr_val != 0 {
        unsafe {
            let _ = Box::from_raw(job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>);
            job.tls_write_future_ptr_val = 0;
        }
    }
    if job.tls_read_future_ptr_val != 0 {
        unsafe {
            let _ = Box::from_raw(job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>);
            job.tls_read_future_ptr_val = 0;
        }
    }
    if job.tls_handshake_context_ptr_val != 0 {
        unsafe {
            let context = Box::from_raw(job.tls_handshake_context_ptr_val as *mut TlsHandshakeContext);
            // The leaked host name is harder to free, we'll let it leak for now as it's small, 
            // or we could track it too. But for a bare-metal OS, a few bytes per redirect is acceptable for now.
            drop(context);
            job.tls_handshake_context_ptr_val = 0;
        }
    }
    finished_jobs.push(idx);
}


fn poll_websocket_jobs(sockets: &mut SocketSet, iface: &mut Interface, current_ticks: u64) {
    let mut finished_jobs = Vec::new();

    {
        let mut jobs = WEBSOCKET_JOBS.lock();
        for (&id, job) in jobs.iter_mut() {
            if current_ticks.saturating_sub(job.start_ticks) > 6000 { 
                serial_println!("[WS] Timeout for {}", job.url);
                job.closed = true;
                finished_jobs.push(id);
                continue;
            }

            match job.state {
                // ... (state transitions as before, just swap job.closed = true; finished_jobs.push(idx); with id)
                0 => { // DHCP Wait
                    if iface.ipv4_addr().is_some() { job.state = 1; }
                }
                1 => { // DNS Search
                    let dns_handle = sockets.iter().find_map(|(h, s)| if matches!(s, smoltcp::socket::Socket::Dns(_)) { Some(h) } else { None }).unwrap();
                    let dns_socket = sockets.get_mut::<DnsSocket>(dns_handle);
                    match dns_socket.start_query(iface.context(), &job.host, DnsQueryType::A) {
                        Ok(qh) => { job.dns_query_handle = Some(qh); job.state = 2; }
                        Err(_) => { job.closed = true; finished_jobs.push(id); }
                    }
                }
                2 => { // DNS Wait
                    let dns_handle = sockets.iter().find_map(|(h, s)| if matches!(s, smoltcp::socket::Socket::Dns(_)) { Some(h) } else { None }).unwrap();
                    let dns_socket = sockets.get_mut::<DnsSocket>(dns_handle);
                    let qh = job.dns_query_handle.unwrap();
                    match dns_socket.get_query_result(qh) {
                        Ok(addrs) => { if let Some(addr) = addrs.first() { job.remote_addr = Some(*addr); job.state = 3; } }
                        Err(GetQueryResultError::Pending) => {}
                        Err(_) => { job.closed = true; finished_jobs.push(id); }
                    }
                }
                3 => { // TCP Connect
                    let tcp_rx = tcp::SocketBuffer::new(vec![0u8; 16384]);
                    let tcp_tx = tcp::SocketBuffer::new(vec![0u8; 16384]);
                    let tcp_socket = TcpSocket::new(tcp_rx, tcp_tx);
                    let handle = sockets.add(tcp_socket);
                    job.tcp_handle = Some(handle);
                    let cx = iface.context();
                    let remote_endpoint = (job.remote_addr.unwrap(), job.port);
                    let local_port = next_local_port();
                    let tcp_socket = sockets.get_mut::<TcpSocket>(handle);
                    match tcp_socket.connect(cx, remote_endpoint, local_port) {
                        Ok(()) => { job.state = 4; }
                        Err(_) => { sockets.remove(handle); job.tcp_handle = None; job.closed = true; finished_jobs.push(id); }
                    }
                }
                4 => { // TCP Wait
                    let handle = job.tcp_handle.unwrap();
                    let tcp_socket = sockets.get_mut::<TcpSocket>(handle);
                    match tcp_socket.state() {
                        State::Established => { if job.is_https { job.state = 5; } else { job.state = 6; } job.last_activity_ticks = current_ticks; }
                        State::Closed | State::CloseWait => { sockets.remove(handle); job.tcp_handle = None; job.closed = true; finished_jobs.push(id); }
                        _ => {}
                    }
                }
                5 => { // TLS Handshake
                    let handle = job.tcp_handle.unwrap();
                    if job.tls_connection_ptr_val == 0 {
                        let read_buf = Box::new([0u8; 16384]);
                        let write_buf = Box::new([0u8; 16384]);
                        let io = NonBlockingSmoltcpIo { handle };
                        let connection: TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256> = TlsConnection::new(io, Box::leak(read_buf), Box::leak(write_buf));
                        job.tls_connection_ptr_val = Box::into_raw(Box::new(connection)) as usize;
                    }
                    if job.tls_handshake_future_ptr_val == 0 {
                        let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                        let rng = rand_chacha::ChaCha20Rng::seed_from_u64(current_ticks + id as u64);
                        let host_leaked = Box::leak(job.host.clone().into_boxed_str());
                        let config = if let Some(ref custom) = job.alpn_protocols {
                            let leaked: alloc::vec::Vec<&'static [u8]> = custom.iter()
                                .map(|s| Box::leak(s.clone().into_bytes().into_boxed_slice()) as &'static [u8])
                                .collect();
                            let leaked_slice: &'static [&'static [u8]] = Box::leak(leaked.into_boxed_slice());
                            TlsConfig::new().with_server_name(host_leaked).with_alpn_protocols(leaked_slice)
                        } else {
                            TlsConfig::new().with_server_name(host_leaked).with_alpn_protocols(&[b"http/1.1"])
                        };
                        let context = Box::leak(Box::new(TlsHandshakeContext { config, rng }));
                        job.tls_handshake_context_ptr_val = context as *mut _ as usize;
                        let future = connection.open::<_, NoVerify>(TlsContext::new(&context.config, &mut context.rng));
                        let boxed_future: Pin<Box<dyn Future<Output = Result<(), TlsError>> + 'static>> = Box::pin(future);
                        job.tls_handshake_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                    }
                    let future_ptr = job.tls_handshake_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>>>>;
                    let future = unsafe { &mut *future_ptr };
                    use futures_util::task::noop_waker_ref;
                    match future.as_mut().poll(&mut Context::from_waker(noop_waker_ref())) {
                        core::task::Poll::Ready(Ok(_)) => { job.state = 6; job.last_activity_ticks = current_ticks; }
                        core::task::Poll::Ready(Err(_)) => { cleanup_ws_job_internal(job, sockets); finished_jobs.push(id); }
                        core::task::Poll::Pending => { if current_ticks.saturating_sub(job.last_activity_ticks) > 500 { cleanup_ws_job_internal(job, sockets); finished_jobs.push(id); } }
                    }
                }
                6 => { // Send Upgrade
                    let upgrade_req = alloc::format!("GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n", job.path, job.host, job.sec_key);
                    if job.is_https {
                        if job.tls_write_future_ptr_val == 0 {
                            let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                            job.tls_send_data = upgrade_req.into_bytes();
                            let data_slice: &'static [u8] = unsafe { core::slice::from_raw_parts(job.tls_send_data.as_ptr(), job.tls_send_data.len()) };
                            let future = connection.write(data_slice);
                            let boxed_future: Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send + 'static>> = Box::pin(future);
                            job.tls_write_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                        }
                        let future_ptr = job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>>>>;
                        let future = unsafe { &mut *future_ptr };
                        use futures_util::task::noop_waker_ref;
                        match future.as_mut().poll(&mut core::task::Context::from_waker(noop_waker_ref())) {
                            core::task::Poll::Ready(Ok(_)) => { unsafe { let _ = Box::from_raw(future_ptr); } job.tls_write_future_ptr_val = 0; job.state = 7; job.last_activity_ticks = current_ticks; }
                            core::task::Poll::Ready(Err(_)) => { cleanup_ws_job_internal(job, sockets); finished_jobs.push(id); }
                            core::task::Poll::Pending => {}
                        }
                    } else {
                        let handle = job.tcp_handle.unwrap();
                        let socket = sockets.get_mut::<TcpSocket>(handle);
                        if socket.can_send() { let _ = socket.send_slice(upgrade_req.as_bytes()); job.state = 7; job.last_activity_ticks = current_ticks; }
                    }
                }
                7 => { // Wait for Upgrade Response
                    let mut data = Vec::new();
                    if job.is_https {
                        if job.tls_read_future_ptr_val == 0 {
                            let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
                            let buffer_slice: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(job.tls_read_buffer.as_mut_ptr(), job.tls_read_buffer.len()) };
                            let future = connection.read(buffer_slice);
                            let boxed_future: Pin<Box<dyn Future<Output = Result<usize, TlsError>> + 'static>> = Box::pin(future);
                            job.tls_read_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
                        }
                        let future_ptr = job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>>>>;
                        let future = unsafe { &mut *future_ptr };
                        use futures_util::task::noop_waker_ref;
                        match future.as_mut().poll(&mut core::task::Context::from_waker(noop_waker_ref())) {
                            core::task::Poll::Ready(Ok(n)) => { unsafe { let _ = Box::from_raw(future_ptr); } job.tls_read_future_ptr_val = 0; if n > 0 { data.extend_from_slice(&job.tls_read_buffer[..n]); } }
                            _ => {}
                        }
                    } else {
                        let handle = job.tcp_handle.unwrap();
                        let socket = sockets.get_mut::<TcpSocket>(handle);
                        if socket.can_recv() {
                            let mut buf = [0u8; 1024];
                            if let Ok(n) = socket.recv_slice(&mut buf) { if n > 0 { data.extend_from_slice(&buf[..n]); } }
                        }
                    }
                    if !data.is_empty() {
                        if let Ok(resp) = core::str::from_utf8(&data) {
                            if resp.contains("101 Switching Protocols") {
                                let expected_accept = base64_encode(&sha1(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", job.sec_key).as_bytes()));
                                if resp.contains(&expected_accept) { job.state = 8; job.last_activity_ticks = current_ticks; } else { cleanup_ws_job_internal(job, sockets); finished_jobs.push(id); }
                            }
                        }
                    }
                }
                8 => { poll_ws_frames(job, sockets, current_ticks); if job.closed { finished_jobs.push(id); } }
                _ => {}
            }
        }
    }

    let mut jobs = WEBSOCKET_JOBS.lock();
    for id in finished_jobs {
        if let Some(mut job) = jobs.remove(&id) {
            cleanup_ws_job_internal(&mut job, sockets);
        }
    }
}

fn cleanup_ws_job_internal(job: &mut WebSocketJob, sockets: &mut SocketSet) {
    if let Some(h) = job.tcp_handle.take() {
        let socket = sockets.get_mut::<TcpSocket>(h);
        socket.abort();
        sockets.remove(h);
    }
    if job.tls_connection_ptr_val != 0 { unsafe { let _ = Box::from_raw(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>); job.tls_connection_ptr_val = 0; } }
    if job.tls_handshake_future_ptr_val != 0 { unsafe { let _ = Box::from_raw(job.tls_handshake_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>> + Send>>); job.tls_handshake_future_ptr_val = 0; } }
    if job.tls_write_future_ptr_val != 0 { unsafe { let _ = Box::from_raw(job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>); job.tls_write_future_ptr_val = 0; } }
    if job.tls_read_future_ptr_val != 0 { unsafe { let _ = Box::from_raw(job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>); job.tls_read_future_ptr_val = 0; } }
    if job.tls_handshake_context_ptr_val != 0 { unsafe { let _ = Box::from_raw(job.tls_handshake_context_ptr_val as *mut TlsHandshakeContext); job.tls_handshake_context_ptr_val = 0; } }
    job.closed = true;
}

fn poll_ws_frames(job: &mut WebSocketJob, sockets: &mut SocketSet, current_ticks: u64) {
    // 1. Send queued messages
    if !job.tx_queue.is_empty() {
        let msg = job.tx_queue.remove(0);
        let payload = msg.as_bytes();
        let mut frame = Vec::new();
        frame.push(0x81); // FIN + Text
        if payload.len() <= 125 {
            frame.push(0x80 | (payload.len() as u8));
        } else if payload.len() <= 65535 {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }

        let mask: [u8; 4] = [
            (current_ticks & 0xFF) as u8,
            ((current_ticks >> 8) & 0xFF) as u8,
            ((current_ticks >> 16) & 0xFF) as u8,
            ((current_ticks >> 24) & 0xFF) as u8,
        ];
        frame.extend_from_slice(&mask);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }

        if job.is_https {
             job.tls_send_data.extend_from_slice(&frame);
        } else {
            let handle = job.tcp_handle.unwrap();
            let socket = sockets.get_mut::<TcpSocket>(handle);
            if socket.can_send() {
                let _ = socket.send_slice(&frame);
            }
        }
    }

    // 1.5 Drive TLS writes if pending
    if job.is_https && !job.tls_send_data.is_empty() && job.tls_write_future_ptr_val == 0 {
        let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
        let data_slice: &'static [u8] = unsafe { core::slice::from_raw_parts(job.tls_send_data.as_ptr(), job.tls_send_data.len()) };
        let future = connection.write(data_slice);
        let boxed_future: Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send + 'static>> = Box::pin(future);
        job.tls_write_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
    }
    if job.tls_write_future_ptr_val != 0 {
        let future_ptr = job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>>>>;
        let future = unsafe { &mut *future_ptr };
        use futures_util::task::noop_waker_ref;
        match future.as_mut().poll(&mut core::task::Context::from_waker(noop_waker_ref())) {
            core::task::Poll::Ready(Ok(n)) => {
                unsafe { let _ = Box::from_raw(future_ptr); }
                job.tls_write_future_ptr_val = 0;
                job.tls_send_data.drain(..n);
            }
            core::task::Poll::Ready(Err(_)) => {
                // Handle error
            }
            core::task::Poll::Pending => {}
        }
    }

    // 2. Receive frames
    let mut data = Vec::new();
    if job.is_https {
        // ... (reuse TLS read logic)
        let connection = unsafe { &mut *(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>) };
        if job.tls_read_future_ptr_val == 0 {
            let buffer_slice: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(job.tls_read_buffer.as_mut_ptr(), job.tls_read_buffer.len()) };
            let future = connection.read(buffer_slice);
            let boxed_future: Pin<Box<dyn Future<Output = Result<usize, TlsError>> + 'static>> = Box::pin(future);
            job.tls_read_future_ptr_val = Box::into_raw(Box::new(boxed_future)) as usize;
        }
        let future_ptr = job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>>>>;
        let future = unsafe { &mut *future_ptr };
        use futures_util::task::noop_waker_ref;
        match future.as_mut().poll(&mut core::task::Context::from_waker(noop_waker_ref())) {
            core::task::Poll::Ready(Ok(n)) => {
                unsafe { let _ = Box::from_raw(future_ptr); }
                job.tls_read_future_ptr_val = 0;
                if n > 0 { data.extend_from_slice(&job.tls_read_buffer[..n]); }
            }
            _ => {}
        }
    } else {
        let handle = job.tcp_handle.unwrap();
        let socket = sockets.get_mut::<TcpSocket>(handle);
        if socket.can_recv() {
            let mut buf = [0u8; 2048];
            if let Ok(n) = socket.recv_slice(&mut buf) {
                if n > 0 { data.extend_from_slice(&buf[..n]); }
            }
        }
    }

    if !data.is_empty() {
        job.frame_buffer.extend_from_slice(&data);
    }

    while job.frame_buffer.len() >= 2 {
        let b0 = job.frame_buffer[0];
        let _fin = (b0 & 0x80) != 0;
        let opcode = b0 & 0x0F;
        let b1 = job.frame_buffer[1];
        let masked = (b1 & 0x80) != 0;
        let mut payload_len = (b1 & 0x7F) as u64;
        let mut header_len = 2;

        if payload_len == 126 {
            if job.frame_buffer.len() < 4 { break; }
            payload_len = u16::from_be_bytes([job.frame_buffer[2], job.frame_buffer[3]]) as u64;
            header_len = 4;
        } else if payload_len == 127 {
            if job.frame_buffer.len() < 10 { break; }
            payload_len = u64::from_be_bytes([
                job.frame_buffer[2], job.frame_buffer[3], job.frame_buffer[4], job.frame_buffer[5],
                job.frame_buffer[6], job.frame_buffer[7], job.frame_buffer[8], job.frame_buffer[9]
            ]);
            header_len = 10;
        }

        let mask_len = if masked { 4 } else { 0 };
        if job.frame_buffer.len() < (header_len + mask_len + payload_len as usize) { break; }

        let payload_start = header_len + mask_len;
        let mut payload = job.frame_buffer[payload_start..payload_start + payload_len as usize].to_vec();
        if masked {
            let mask = [
                job.frame_buffer[header_len], job.frame_buffer[header_len+1],
                job.frame_buffer[header_len+2], job.frame_buffer[header_len+3]
            ];
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }

        // Consume consumed bytes
        job.frame_buffer.drain(..payload_start + payload_len as usize);

        match opcode {
            0x1 => { // Text
                if let Ok(s) = String::from_utf8(payload) {
                    job.rx_queue.push(s);
                }
            }
            0x8 => { // Close
                job.closed = true;
            }
            0x9 => { // Ping
                // Send Pong (opcode 0xA)
                let pong = [0x8A, 0x00];
                if job.is_https {
                    job.tls_send_data.extend_from_slice(&pong);
                } else {
                    let socket = sockets.get_mut::<TcpSocket>(job.tcp_handle.unwrap());
                    let _ = socket.send_slice(&pong);
                }
            }
            _ => {}
        }
    }
}

fn handle_redirect_or_finish(idx: usize, job: &mut FetchJob, sockets: &mut SocketSet, finished_jobs: &mut alloc::vec::Vec<usize>) {
    // Basic redirect check: 3xx + Location header
    let first_line = job.response.lines().next().unwrap_or("");
    let is_redirect = (first_line.contains(" 301") || first_line.contains(" 302") || first_line.contains(" 303") || first_line.contains(" 307") || first_line.contains(" 308"))
        && first_line.starts_with("HTTP/");
    let has_location = job.response.contains("Location:") || job.response.contains("location:");
    if is_redirect && has_location && job.redirect_count < 5 {
        if let Some(loc_line) = job.response.lines().find(|l| l.starts_with("Location:") || l.starts_with("location:")) {
            let colon_pos = loc_line.find(':').unwrap_or(0);
            let new_url = loc_line[colon_pos + 1..].trim();
            serial_println!("[FETCH] Redirecting to: {}", new_url);
            
            // Re-parse URL and reset job
            let (is_https, host, port, path) = parse_url(new_url);
            job.url = new_url.into();
            job.host = host;
            job.port = port;
            job.path = path;
            job.is_https = is_https;
            job.redirect_count += 1;
            job.response.clear();
            job.state = 1; // back to DNS
            
            // Cleanup current socket
            if let Some(handle) = job.tcp_handle {
                let socket = sockets.get_mut::<TcpSocket>(handle);
                socket.abort();
                sockets.remove(handle);
                job.tcp_handle = None;
            }
            if job.tls_connection_ptr_val != 0 {
                unsafe {
                     let _ = Box::from_raw(job.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>);
                     job.tls_connection_ptr_val = 0;
                }
            }
            if job.tls_handshake_future_ptr_val != 0 {
                unsafe {
                    let _ = Box::from_raw(job.tls_handshake_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>> + Send>>);
                    job.tls_handshake_future_ptr_val = 0;
                }
            }
            if job.tls_write_future_ptr_val != 0 {
                unsafe {
                    let _ = Box::from_raw(job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>);
                    job.tls_write_future_ptr_val = 0;
                }
            }
            if job.tls_read_future_ptr_val != 0 {
                unsafe {
                    let _ = Box::from_raw(job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>);
                    job.tls_read_future_ptr_val = 0;
                }
            }
            if job.tls_write_future_ptr_val != 0 {
                unsafe {
                    let _ = Box::from_raw(job.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>);
                    job.tls_write_future_ptr_val = 0;
                }
            }
            if job.tls_read_future_ptr_val != 0 {
                unsafe {
                    let _ = Box::from_raw(job.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>);
                    job.tls_read_future_ptr_val = 0;
                }
            }
            if job.tls_handshake_context_ptr_val != 0 {
                unsafe {
                    let _ = Box::from_raw(job.tls_handshake_context_ptr_val as *mut TlsHandshakeContext);
                    job.tls_handshake_context_ptr_val = 0;
                }
            }
            return;
        }
    }

    cleanup_job(job, sockets, finished_jobs, idx);
}

fn parse_url(url: &str) -> (bool, alloc::string::String, u16, alloc::string::String) {
    let is_https = url.starts_with("https://");
    let url_strip = if is_https { &url[8..] } else if url.starts_with("http://") { &url[7..] } else { url };
    
    let slash_idx = url_strip.find('/').unwrap_or(url_strip.len());
    let host_and_port = &url_strip[..slash_idx];
    let path = if slash_idx < url_strip.len() { &url_strip[slash_idx..] } else { "/" };

    let mut host = host_and_port.to_string();
    let mut port = if is_https { 443 } else { 80 };

    if let Some(colon_idx) = host_and_port.find(':') {
        host = host_and_port[..colon_idx].to_string();
        if let Ok(p) = host_and_port[colon_idx+1..].parse::<u16>() {
            port = p;
        }
    }
    
    (is_https, host, port, path.into())
}

fn parse_headers_simple(json: &str) -> alloc::vec::Vec<(alloc::string::String, alloc::string::String)> {
    let mut headers = alloc::vec::Vec::new();
    let trimmed = json.trim().trim_matches(|c| c == '{' || c == '}');
    if trimmed.is_empty() { return headers; }
    
    // Very naive split. Handles basic {"Key":"Val","Key2":"Val2"}
    for pair in trimmed.split(',') {
        let parts: alloc::vec::Vec<&str> = pair.splitn(2, ':').collect();
        if parts.len() == 2 {
            let key = parts[0].trim().trim_matches('"');
            let val = parts[1].trim().trim_matches('"');
            headers.push((alloc::string::String::from(key), alloc::string::String::from(val)));
        }
    }
    headers
}

fn poll_server_sockets(sockets: &mut SocketSet, current_ticks: u64) {
    // 1. Check for new connections on listening sockets
    {
        let mut listeners = LISTENERS.lock();
        for listener in listeners.iter_mut() {
            let socket = sockets.get_mut::<TcpSocket>(listener.handle);
            if socket.is_active() && socket.state() == tcp::State::Established {
                let old_handle = listener.handle;
                
                // Replace the listener with a fresh socket immediately
                let mut new_socket = TcpSocket::new(
                    tcp::SocketBuffer::new(vec![0; 32768]),
                    tcp::SocketBuffer::new(vec![0; 32768])
                );
                let _ = new_socket.listen(listener.port);
                listener.handle = sockets.add(new_socket);

                // Add the established connection as a job
                SERVER_JOBS.lock().push(Box::new(ServerJob {
                    pid: listener.pid,
                    handle: old_handle,
                    buffer: vec![],
                    response: None,
                    bytes_sent: 0,
                    start_ticks: current_ticks,
                }));
            }
        }
    }

    // 2. Drive ServerJobs
    let mut server_jobs = SERVER_JOBS.lock();
    let mut i = 0;
    while i < server_jobs.len() {
        let job = &mut server_jobs[i];
        let socket = sockets.get_mut::<TcpSocket>(job.handle);
        
        let mut finished = false;

        if socket.can_recv() {
            let mut buf = [0u8; 1024];
            if let Ok(n) = socket.recv_slice(&mut buf) {
                if n > 0 {
                    job.buffer.extend_from_slice(&buf[..n]);
                    if let Ok(req) = core::str::from_utf8(&job.buffer) {
                        if req.contains("\r\n\r\n") {
                            let response = if req.contains("GET /screenshot") {
                                let data = crate::framebuffer::get_screenshot_bmp_small();
                                let mut res = alloc::format!("HTTP/1.1 200 OK\r\nContent-Type: image/bmp\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", data.len()).into_bytes();
                                res.extend_from_slice(&data);
                                Some(res)
                            } else if req.contains("GET /key") {
                                if let Some(code_idx) = req.find("code=") {
                                    let code_str = &req[code_idx+5..];
                                    let end_idx = code_str.find(|c: char| !c.is_numeric()).unwrap_or(code_str.len());
                                    if let Ok(code) = code_str[..end_idx].parse::<u32>() {
                                        let character = match code {
                                            13 => Some('\n'),
                                            8 => Some('\x08'),
                                            32 => Some(' '),
                                            c if c >= 65 && c <= 90 => Some((c as u8).to_ascii_lowercase() as char),
                                            c if c >= 48 && c <= 57 => Some((c as u8) as char),
                                            _ => None,
                                        };
                                        if let Some(c) = character {
                                            crate::shell::handle_key(c);
                                        }
                                    }
                                }
                                Some("HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\nOK".to_string().into_bytes())
                            } else {
                                let custom = crate::net::CUSTOM_HTTP_RESPONSE.lock().clone();
                                if let Some(html) = custom {
                                    Some(alloc::format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{}", html).into_bytes())
                                } else {
                                    Some("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n404 Not Found".to_string().into_bytes())
                                }
                            };
                            job.response = response;
                        }
                    }
                }
            }
        }

        if let Some(ref mut resp) = job.response {
            if socket.can_send() {
                let remaining = &resp[job.bytes_sent..];
                if let Ok(n) = socket.send_slice(remaining) {
                    job.bytes_sent += n;
                    if job.bytes_sent >= resp.len() {
                        job.response = None;
                        socket.close();
                    }
                }
            }
        }

        if !socket.is_active() || !socket.is_open() || current_ticks.saturating_sub(job.start_ticks) > 1000 {
            finished = true;
        }

        if finished {
            sockets.remove(job.handle);
            server_jobs.remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn cleanup_process_network(pid: u32) {
    let mut sockets_guard = (*SOCKETS).lock();
    if sockets_guard.is_none() { return; }
    let sockets = sockets_guard.as_mut().unwrap();

    let mut fetch_jobs = FETCH_JOBS.lock();
    let mut i = 0;
    while i < fetch_jobs.len() {
        if fetch_jobs[i].pid == pid {
            let mut job_box = fetch_jobs.remove(i);
            if let Some(handle) = job_box.tcp_handle {
                let socket = sockets.get_mut::<TcpSocket>(handle);
                socket.abort();
                sockets.remove(handle);
            }
            if job_box.tls_connection_ptr_val != 0 { unsafe { let _ = Box::from_raw(job_box.tls_connection_ptr_val as *mut TlsConnection<'static, NonBlockingSmoltcpIo, Aes128GcmSha256>); } }
            if job_box.tls_handshake_future_ptr_val != 0 { unsafe { let _ = Box::from_raw(job_box.tls_handshake_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<(), TlsError>> + Send>>); } }
            if job_box.tls_read_future_ptr_val != 0 { unsafe { let _ = Box::from_raw(job_box.tls_read_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>); } }
            if job_box.tls_write_future_ptr_val != 0 { unsafe { let _ = Box::from_raw(job_box.tls_write_future_ptr_val as *mut Pin<Box<dyn Future<Output = Result<usize, TlsError>> + Send>>); } }
            if job_box.tls_handshake_context_ptr_val != 0 { unsafe { let _ = Box::from_raw(job_box.tls_handshake_context_ptr_val as *mut TlsHandshakeContext); } }
        } else {
            i += 1;
        }
    }

    let mut ws_jobs = WEBSOCKET_JOBS.lock();
    let mut ws_to_remove = alloc::vec::Vec::new();
    for (id, job) in ws_jobs.iter_mut() {
        if job.pid == pid {
            cleanup_ws_job_internal(job, sockets);
            ws_to_remove.push(*id);
        }
    }
    for id in ws_to_remove { ws_jobs.remove(&id); }

    let mut server_jobs = SERVER_JOBS.lock();
    let mut i = 0;
    while i < server_jobs.len() {
        if server_jobs[i].pid == pid {
            let job = server_jobs.remove(i);
            let socket = sockets.get_mut::<TcpSocket>(job.handle);
            socket.abort();
            sockets.remove(job.handle);
        } else {
            i += 1;
        }
    }

    let mut listeners = LISTENERS.lock();
    let mut i = 0;
    while i < listeners.len() {
        if listeners[i].pid == pid {
            let l = listeners.remove(i);
            let socket = sockets.get_mut::<TcpSocket>(l.handle);
            socket.abort();
            sockets.remove(l.handle);
        } else {
            i += 1;
        }
    }

    // Clean up TFTP jobs
    let mut tftp_jobs = TFTP_JOBS.lock();
    let mut i = 0;
    while i < tftp_jobs.len() {
        if tftp_jobs[i].pid == pid {
            let job = tftp_jobs.remove(i);
            // Only remove the socket if the job hasn't already cleaned it up
            if !job.done {
                let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(job.udp_handle);
                socket.close();
                sockets.remove(job.udp_handle);
            }
        } else {
            i += 1;
        }
    }

    // Clean up FTP sessions
    ftp_job::cleanup_process_ftp(pid, sockets);
}
