//! Pure-Rust client for `Karabiner-VirtualHIDDevice-Daemon`.
//!
//! Automatically supports both:
//! - Protocol 7 (DriverKit v8.x+): Unix domain stream socket at
//!   `/Library/Application Support/org.pqrs/tmp/rootonly/karabiner_virtual_hid_device_service.sock`
//! - Protocol 5 (DriverKit v6.x, e.g. v6.14.0): Unix domain datagram socket in
//!   `/Library/Application Support/org.pqrs/tmp/rootonly/vhidd_server/*.sock`

use crate::hid::HidReport;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const ROOTONLY_DIR: &str = "/Library/Application Support/org.pqrs/tmp/rootonly";
pub const V7_SERVER_SOCKET: &str =
    "/Library/Application Support/org.pqrs/tmp/rootonly/karabiner_virtual_hid_device_service.sock";
pub const V5_SERVER_SOCKET_DIR: &str =
    "/Library/Application Support/org.pqrs/tmp/rootonly/vhidd_server";
pub const V5_CLIENT_SOCKET_DIR: &str =
    "/Library/Application Support/org.pqrs/tmp/rootonly/vhidd_client";

pub const VIRTUAL_VENDOR_ID: u64 = 0x16c0;
pub const VIRTUAL_PRODUCT_ID: u64 = 0x27db;
pub const VIRTUAL_COUNTRY_CODE: u64 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    V5,
    V7,
}

#[derive(Clone, Copy, Debug)]
enum RequestOp {
    KeyboardInit,
    KeyboardTerminate,
    KeyboardReset,
    PostKeyboard,
    PostConsumer,
    PostAppleVendorKeyboard,
    PostAppleVendorTopCase,
    PostGenericDesktop,
}

impl RequestOp {
    fn code_v5(self) -> u8 {
        match self {
            Self::KeyboardInit => 1,
            Self::KeyboardTerminate => 2,
            Self::KeyboardReset => 3,
            Self::PostKeyboard => 7,
            Self::PostConsumer => 8,
            Self::PostAppleVendorKeyboard => 9,
            Self::PostAppleVendorTopCase => 10,
            Self::PostGenericDesktop => 11,
        }
    }

    fn code_v7(self) -> u8 {
        match self {
            Self::KeyboardInit => 0,
            Self::KeyboardTerminate => 1,
            Self::KeyboardReset => 2,
            Self::PostKeyboard => 6,
            Self::PostConsumer => 7,
            Self::PostAppleVendorKeyboard => 8,
            Self::PostAppleVendorTopCase => 9,
            Self::PostGenericDesktop => 10,
        }
    }
}

fn keyboard_parameters_bytes() -> [u8; 24] {
    let mut out = [0u8; 24];
    out[0..8].copy_from_slice(&VIRTUAL_VENDOR_ID.to_le_bytes());
    out[8..16].copy_from_slice(&VIRTUAL_PRODUCT_ID.to_le_bytes());
    out[16..24].copy_from_slice(&VIRTUAL_COUNTRY_CODE.to_le_bytes());
    out
}

fn encode_v5_packet(op: RequestOp, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6 + payload.len());
    buf.push(1u8); // pqrs::local_datagram::impl::send_entry::type::user_data
    buf.push(b'c');
    buf.push(b'p');
    buf.extend_from_slice(&5u16.to_le_bytes());
    buf.push(op.code_v5());
    buf.extend_from_slice(payload);
    buf
}

fn encode_v5_heartbeat(deadline_ms: u32) -> [u8; 5] {
    let mut buf = [0u8; 5];
    buf[0] = 0u8; // pqrs::local_datagram::impl::send_entry::type::heartbeat
    buf[1..5].copy_from_slice(&deadline_ms.to_le_bytes());
    buf
}

fn encode_v7_request_frame(request_id: u64, op: RequestOp, payload: &[u8]) -> Vec<u8> {
    let inner_len = 2 + 1 + payload.len();
    let body_len = (1 + 8 + inner_len) as u32;
    let mut buf = Vec::with_capacity(4 + body_len as usize);
    buf.extend_from_slice(&body_len.to_be_bytes());
    buf.push(4u8); // message_type::request
    buf.extend_from_slice(&request_id.to_be_bytes());
    buf.extend_from_slice(&7u16.to_le_bytes());
    buf.push(op.code_v7());
    buf.extend_from_slice(payload);
    buf
}

fn encode_v7_response_frame(request_id: u64) -> [u8; 13] {
    let mut buf = [0u8; 13];
    buf[0..4].copy_from_slice(&9u32.to_be_bytes());
    buf[4] = 5u8; // message_type::response
    buf[5..13].copy_from_slice(&request_id.to_be_bytes());
    buf
}

fn encode_v7_simple_frame(msg_type: u8) -> [u8; 5] {
    let mut buf = [0u8; 5];
    buf[0..4].copy_from_slice(&1u32.to_be_bytes());
    buf[4] = msg_type;
    buf
}

enum Transport {
    V5 {
        socket: UnixDatagram,
        client_path: PathBuf,
    },
    V7 {
        writer: Arc<Mutex<UnixStream>>,
        next_req_id: AtomicU64,
    },
}

pub struct VirtualKeyboardClient {
    transport: Transport,
    stop: Arc<AtomicBool>,
    keyboard_ready: Arc<AtomicBool>,
    verbose: bool,
}

impl VirtualKeyboardClient {
    /// Connect to `Karabiner-VirtualHIDDevice-Daemon`, auto-detecting Protocol 7 vs Protocol 5
    /// unless `forced_protocol` is provided.
    pub fn connect(forced_protocol: Option<Protocol>, verbose: bool) -> io::Result<Self> {
        let protocol = match forced_protocol {
            Some(p) => p,
            None => detect_protocol()?,
        };

        let client = match protocol {
            Protocol::V7 => Self::connect_v7(verbose)?,
            Protocol::V5 => Self::connect_v5(verbose)?,
        };

        client.initialize_keyboard()?;
        Ok(client)
    }

    pub fn protocol(&self) -> Protocol {
        match self.transport {
            Transport::V5 { .. } => Protocol::V5,
            Transport::V7 { .. } => Protocol::V7,
        }
    }

    pub fn is_keyboard_ready(&self) -> bool {
        self.keyboard_ready.load(Ordering::Relaxed)
    }

    fn connect_v5(verbose: bool) -> io::Result<Self> {
        let server_path = find_v5_server_socket()?;
        fs::create_dir_all(V5_CLIENT_SOCKET_DIR)?;

        let ts_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let client_path = PathBuf::from(V5_CLIENT_SOCKET_DIR)
            .join(format!("ct_{}_{:x}.sock", std::process::id(), ts_ns));
        let _ = fs::remove_file(&client_path);

        let socket = UnixDatagram::bind(&client_path)?;
        socket.connect(&server_path)?;
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;

        if verbose {
            eprintln!(
                "[controlesque] connected to vhidd (protocol 5) via {}",
                server_path.display()
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let keyboard_ready = Arc::new(AtomicBool::new(false));

        let bg_socket = socket.try_clone()?;
        let bg_stop = Arc::clone(&stop);
        let bg_ready = Arc::clone(&keyboard_ready);

        // Background thread: sends periodic heartbeats and reads daemon status responses.
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut ticks = 0u32;
            while !bg_stop.load(Ordering::Relaxed) {
                if ticks % 4 == 0 {
                    let _ = bg_socket.send(&encode_v5_heartbeat(5000));
                }
                ticks = ticks.wrapping_add(1);

                match bg_socket.recv(&mut buf) {
                    Ok(n) if n >= 3 && buf[0] == 1 => {
                        handle_response_pair(buf[1], buf[2], &bg_ready, verbose);
                    }
                    Ok(_) => {}
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut => {}
                    Err(_) => {
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        });

        Ok(Self {
            transport: Transport::V5 {
                socket,
                client_path,
            },
            stop,
            keyboard_ready,
            verbose,
        })
    }

    fn connect_v7(verbose: bool) -> io::Result<Self> {
        let stream = UnixStream::connect(V7_SERVER_SOCKET)?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        let reader_stream = stream.try_clone()?;
        let writer = Arc::new(Mutex::new(stream));

        if verbose {
            eprintln!(
                "[controlesque] connected to vhidd (protocol 7) via {}",
                V7_SERVER_SOCKET
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let keyboard_ready = Arc::new(AtomicBool::new(false));

        let bg_writer = Arc::clone(&writer);
        let bg_stop = Arc::clone(&stop);
        let bg_ready = Arc::clone(&keyboard_ready);

        thread::spawn(move || {
            let mut reader = reader_stream;
            let mut header = [0u8; 4];
            let mut ticks = 0u32;

            while !bg_stop.load(Ordering::Relaxed) {
                if ticks % 20 == 0 {
                    if let Ok(mut w) = bg_writer.lock() {
                        let _ = w.write_all(&encode_v7_simple_frame(0)); // heartbeat
                    }
                }
                ticks = ticks.wrapping_add(1);

                match reader.read_exact(&mut header) {
                    Ok(()) => {
                        let body_len = u32::from_be_bytes(header) as usize;
                        if body_len == 0 || body_len > 4096 {
                            continue;
                        }
                        let mut body = vec![0u8; body_len];
                        if reader.read_exact(&mut body).is_err() {
                            break;
                        }
                        match body[0] {
                            2 => {
                                // health_check -> reply with health_check_response (3)
                                if let Ok(mut w) = bg_writer.lock() {
                                    let _ = w.write_all(&encode_v7_simple_frame(3));
                                }
                            }
                            4 if body.len() >= 9 => {
                                // server request -> process pairs and send empty response
                                let req_id = u64::from_be_bytes(
                                    body[1..9].try_into().unwrap_or([0; 8]),
                                );
                                let payload = &body[9..];
                                for pair in payload.chunks_exact(2) {
                                    handle_response_pair(pair[0], pair[1], &bg_ready, verbose);
                                }
                                if let Ok(mut w) = bg_writer.lock() {
                                    let _ = w.write_all(&encode_v7_response_frame(req_id));
                                }
                            }
                            5 if body.len() >= 9 => {
                                // response to one of our requests
                                let payload = &body[9..];
                                for pair in payload.chunks_exact(2) {
                                    handle_response_pair(pair[0], pair[1], &bg_ready, verbose);
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut => {}
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            transport: Transport::V7 {
                writer,
                next_req_id: AtomicU64::new(1),
            },
            stop,
            keyboard_ready,
            verbose,
        })
    }

    pub fn initialize_keyboard(&self) -> io::Result<()> {
        let params = keyboard_parameters_bytes();
        self.send_op(RequestOp::KeyboardInit, &params)
    }

    pub fn reset_keyboard(&self) -> io::Result<()> {
        self.send_op(RequestOp::KeyboardReset, &[])
    }

    pub fn terminate_keyboard(&self) -> io::Result<()> {
        self.send_op(RequestOp::KeyboardTerminate, &[])
    }

    pub fn post_report(&self, report: HidReport) -> io::Result<()> {
        match report {
            HidReport::Keyboard(r) => {
                let bytes = r.to_bytes();
                self.send_op(RequestOp::PostKeyboard, &bytes)
            }
            HidReport::Consumer(r) => {
                let bytes = r.to_bytes(2);
                self.send_op(RequestOp::PostConsumer, &bytes)
            }
            HidReport::AppleVendorTopCase(r) => {
                let bytes = r.to_bytes(3);
                self.send_op(RequestOp::PostAppleVendorTopCase, &bytes)
            }
            HidReport::AppleVendorKeyboard(r) => {
                let bytes = r.to_bytes(4);
                self.send_op(RequestOp::PostAppleVendorKeyboard, &bytes)
            }
            HidReport::GenericDesktop(r) => {
                let bytes = r.to_bytes(7);
                self.send_op(RequestOp::PostGenericDesktop, &bytes)
            }
        }
    }

    fn send_op(&self, op: RequestOp, payload: &[u8]) -> io::Result<()> {
        match &self.transport {
            Transport::V5 { socket, .. } => {
                let pkt = encode_v5_packet(op, payload);
                socket.send(&pkt)?;
                Ok(())
            }
            Transport::V7 {
                writer,
                next_req_id,
            } => {
                let req_id = next_req_id.fetch_add(1, Ordering::Relaxed);
                let frame = encode_v7_request_frame(req_id, op, payload);
                let mut w = writer
                    .lock()
                    .map_err(|_| io::Error::other("vhidd writer lock poisoned"))?;
                w.write_all(&frame)?;
                Ok(())
            }
        }
    }
}

impl Drop for VirtualKeyboardClient {
    fn drop(&mut self) {
        let _ = self.reset_keyboard();
        let _ = self.terminate_keyboard();
        self.stop.store(true, Ordering::Relaxed);
        if let Transport::V5 { client_path, .. } = &self.transport {
            let _ = fs::remove_file(client_path);
        }
        if self.verbose {
            eprintln!("[controlesque] disconnected from vhidd");
        }
    }
}

fn handle_response_pair(resp: u8, val: u8, ready: &AtomicBool, verbose: bool) {
    let flag = val != 0;
    match resp {
        1 => {
            if verbose {
                eprintln!("[controlesque] vhidd: driver_activated={flag}");
            }
        }
        2 => {
            if verbose {
                eprintln!("[controlesque] vhidd: driver_connected={flag}");
            }
        }
        3 => {
            if flag {
                eprintln!("[controlesque] WARNING: DriverKit version mismatch reported by daemon");
            }
        }
        4 => {
            ready.store(flag, Ordering::Relaxed);
            if verbose {
                eprintln!("[controlesque] vhidd: virtual_hid_keyboard_ready={flag}");
            }
        }
        _ => {}
    }
}

fn detect_protocol() -> io::Result<Protocol> {
    if Path::new(V7_SERVER_SOCKET).exists() {
        return Ok(Protocol::V7);
    }
    if find_v5_server_socket().is_ok() {
        return Ok(Protocol::V5);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "Karabiner-VirtualHIDDevice-Daemon socket not found under {ROOTONLY_DIR}. \
             Make sure Karabiner-DriverKit-VirtualHIDDevice is installed, activated, \
             Karabiner-VirtualHIDDevice-Daemon is running, and controlesque is run with sudo."
        ),
    ))
}

fn find_v5_server_socket() -> io::Result<PathBuf> {
    let mut socks: Vec<PathBuf> = fs::read_dir(V5_SERVER_SOCKET_DIR)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sock"))
        .collect();
    socks.sort();
    socks.pop().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no *.sock found in {V5_SERVER_SOCKET_DIR}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v5_packet_encoding_matches_pqrs_spec() {
        let params = keyboard_parameters_bytes();
        let pkt = encode_v5_packet(RequestOp::KeyboardInit, &params);
        // [0x01, 'c', 'p', 5, 0, 1, vendor_id(8), product_id(8), country_code(8)]
        assert_eq!(pkt.len(), 30);
        assert_eq!(&pkt[0..6], &[1, b'c', b'p', 5, 0, 1]);
        assert_eq!(&pkt[6..14], &VIRTUAL_VENDOR_ID.to_le_bytes());
        assert_eq!(&pkt[14..22], &VIRTUAL_PRODUCT_ID.to_le_bytes());
        assert_eq!(&pkt[22..30], &VIRTUAL_COUNTRY_CODE.to_le_bytes());
    }

    #[test]
    fn v7_request_frame_encoding_matches_pqrs_spec() {
        let frame = encode_v7_request_frame(42, RequestOp::KeyboardReset, &[]);
        // body_len = 1 (type) + 8 (req_id) + 2 (proto) + 1 (op) = 12
        assert_eq!(frame.len(), 16);
        assert_eq!(&frame[0..4], &12u32.to_be_bytes());
        assert_eq!(frame[4], 4); // request
        assert_eq!(&frame[5..13], &42u64.to_be_bytes());
        assert_eq!(&frame[13..15], &7u16.to_le_bytes());
        assert_eq!(frame[15], 2); // KeyboardReset in v7
    }
}
