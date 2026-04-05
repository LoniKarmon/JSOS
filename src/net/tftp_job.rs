// src/net/tftp_job.rs — TFTP job: wires TftpClient state machine to smoltcp UDP sockets.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::format;
use smoltcp::iface::SocketHandle;
use smoltcp::iface::SocketSet;
use smoltcp::socket::udp::{self, Socket as UdpSocket};
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
    /// Starts as 69; updated to the server's ephemeral port after first response.
    pub server_port: u16,
    pub start_ticks: u64,
    pub is_write: bool,
    pub done: bool,
    pub result: Option<String>, // "OK:<bytes>" or "Error: <msg>"
}

/// Start a TFTP GET: download `remote_path` from `server_ip` and save to JSKV `store_key`.
pub fn start_tftp_get(
    pid: u32,
    server_ip: &str,
    remote_path: &str,
    store_key: &str,
    sockets: &mut SocketSet<'static>,
) {
    let addr = match parse_ipv4(server_ip) {
        Some(a) => a,
        None => {
            serial_println!("[TFTP] Invalid server IP: {}", server_ip);
            return;
        }
    };
    let server_addr = IpAddress::Ipv4(addr);

    let (client, initial_packet) = TftpClient::start_read(remote_path);

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
    let addr = match parse_ipv4(server_ip) {
        Some(a) => a,
        None => {
            serial_println!("[TFTP] Invalid server IP: {}", server_ip);
            return;
        }
    };
    let server_addr = IpAddress::Ipv4(addr);

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

/// Poll all active TFTP jobs. Called from `poll_network()` while `sockets` is already locked.
pub fn poll_tftp_jobs(sockets: &mut SocketSet, current_ticks: u64) {
    let mut jobs = super::TFTP_JOBS.lock();
    let mut i = 0;
    while i < jobs.len() {
        let job = &mut *jobs[i];

        // Timeout after ~30 seconds (100 ticks/sec * 30 = 3000)
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

        // Use raw pointer so we can borrow `sockets` independently from `job`
        let job_ptr = job as *mut TftpJob;

        let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
        if let Ok((data, remote_endpoint)) = socket.recv() {
            let job = unsafe { &mut *job_ptr };
            // TFTP servers respond from an ephemeral port — track it for subsequent sends
            job.server_port = remote_endpoint.endpoint.port;

            // We need the data as an owned slice for receive(); copy it out first
            let data_owned: alloc::vec::Vec<u8> = data.to_vec();

            match job.client.receive(&data_owned) {
                TftpAction::Send(pkt) => {
                    let endpoint = IpEndpoint::new(job.server_addr, job.server_port);
                    let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
                    let _ = socket.send_slice(&pkt, endpoint);

                    // For read transfers: is_complete() becomes true after the final ACK Send
                    if job.client.is_complete() {
                        let file_data = job.client.take_data();
                        storage::write_object(&job.store_key, &file_data);
                        serial_println!(
                            "[TFTP] GET complete: {} bytes -> store:{}",
                            file_data.len(),
                            job.store_key
                        );
                        job.result = Some(format!("OK:{}", file_data.len()));
                        job.done = true;
                        let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
                        socket.close();
                        sockets.remove(job.udp_handle);
                    }
                }
                TftpAction::Complete(_) => {
                    // Write transfer: server ACK'd our last DATA block
                    serial_println!("[TFTP] PUT complete: store:{}", job.store_key);
                    job.result = Some(String::from("OK"));
                    job.done = true;
                    let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
                    socket.close();
                    sockets.remove(job.udp_handle);
                }
                TftpAction::Error(e) => {
                    serial_println!("[TFTP] Protocol error: {:?}", e);
                    job.result = Some(format!("Error: {:?}", e));
                    job.done = true;
                    let socket = sockets.get_mut::<UdpSocket>(job.udp_handle);
                    socket.close();
                    sockets.remove(job.udp_handle);
                }
            }
        }

        i += 1;
    }

    // Remove finished jobs
    jobs.retain(|j| !j.done);
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
