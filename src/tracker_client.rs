use std::collections::hash_map::DefaultHasher;
use std::convert::TryInto;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::Error;
use std::io::ErrorKind::TimedOut;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, SystemTime};

use tokio::net::UdpSocket;
use tokio::time;
use tokio::time::error::Elapsed;

use self::UdpTrackerClientError::{ApplicationError, GeneralError};

const CONNECT_ID_PROTOCOL_ID: u64 = 0x41727101980;

const ACTION_CONNECT: u32 = 0;
const ACTION_ANNOUNCE: u32 = 1;
const ACTION_ERROR: u32 = 3;

const EVENT_NONE: u32 = 0;
const EVENT_STARTED: u32 = 2;
const EVENT_STOPPED: u32 = 3;

pub struct UdpTrackerClient<'a> {
    socket: &'a UdpSocket,
    tracker_addr: &'a SocketAddr,
    conn_id: u64,
    timeout: Duration,
}

pub struct AnnounceResponse {
    pub interval: i32,
    pub leechers: i32,
    pub seeders: i32,
    pub peers: Vec<SocketAddr>,
}

impl<'a> UdpTrackerClient<'a> {
    pub fn new(socket: &'a UdpSocket, tracker_addr: &'a SocketAddr) -> Self {
        Self {
            socket,
            tracker_addr,
            conn_id: 0,
            timeout: Duration::from_secs(5),
        }
    }

    pub async fn connect(&mut self) -> UdpTrackerClientResult<()> {
        let transaction_id = UdpTrackerClient::create_random_transaction_id();

        let mut request = [0u8; 16];
        request[0..8].copy_from_slice(&CONNECT_ID_PROTOCOL_ID.to_be_bytes());
        request[8..12].copy_from_slice(&ACTION_CONNECT.to_be_bytes());
        request[12..16].copy_from_slice(&transaction_id.to_be_bytes());

        let sent = self.socket.send_to(&request, self.tracker_addr).await?;
        if sent != request.len() {
            return Err(GeneralError("Failed to send the entire CONNECT request"));
        }

        let mut buffer = [0u8; 1024];
        let (read, source) =
            time::timeout(self.timeout, self.socket.recv_from(&mut buffer)).await??;

        if source != *self.tracker_addr {
            return Err(ApplicationError(
                "CONNECT response came from an unexpected address",
            ));
        }

        if read < 8 {
            return Err(ApplicationError("Incomplete CONNECT response"));
        }

        let action = u32::from_be_bytes(buffer[0..4].try_into().unwrap());
        let response_transaction_id =
            u32::from_be_bytes(buffer[4..8].try_into().unwrap());

        if response_transaction_id != transaction_id {
            return Err(ApplicationError(
                "CONNECT response has an unexpected transaction ID",
            ));
        }

        match action {
            ACTION_CONNECT => {
                if read < 16 {
                    return Err(ApplicationError("Incomplete CONNECT response"));
                }

                self.conn_id =
                    u64::from_be_bytes(buffer[8..16].try_into().unwrap());

                Ok(())
            }

            ACTION_ERROR => {
                Err(ApplicationError("Tracker returned an error to CONNECT"))
            }

            _ => Err(ApplicationError(
                "Expected CONNECT response, got another response type",
            )),
        }
    }

    pub async fn announce(
        &self,
        info_hash: &[u8; 20],
        peer_id: &[u8; 20],
        downloaded: u64,
        left: u64,
        uploaded: u64,
        event: AnnounceEvent,
        port: u16,
    ) -> UdpTrackerClientResult<AnnounceResponse> {
        if self.conn_id == 0 {
            return Err(ApplicationError("You have to run connect first!"));
        }

        let transaction_id = UdpTrackerClient::create_random_transaction_id();

        /*
         * BEP 15 announce request:
         *
         *  0  connection_id  64-bit
         *  8  action         32-bit
         * 12  transaction_id 32-bit
         * 16  info_hash      20 bytes
         * 36  peer_id        20 bytes
         * 56  downloaded     64-bit
         * 64  left           64-bit
         * 72  uploaded       64-bit
         * 80  event          32-bit
         * 84  IP address     32-bit
         * 88  key            32-bit
         * 92  num_want       32-bit
         * 96  port           16-bit
         */

        let mut request = [0u8; 98];

        request[0..8].copy_from_slice(&self.conn_id.to_be_bytes());
        request[8..12].copy_from_slice(&ACTION_ANNOUNCE.to_be_bytes());
        request[12..16].copy_from_slice(&transaction_id.to_be_bytes());
        request[16..36].copy_from_slice(info_hash);
        request[36..56].copy_from_slice(peer_id);
        request[56..64].copy_from_slice(&downloaded.to_be_bytes());
        request[64..72].copy_from_slice(&left.to_be_bytes());
        request[72..80].copy_from_slice(&uploaded.to_be_bytes());
        request[80..84].copy_from_slice(&(event as u32).to_be_bytes());

        // Zero means that the tracker should use the source IP address.
        request[84..88].copy_from_slice(&0u32.to_be_bytes());

        request[88..92]
            .copy_from_slice(&UdpTrackerClient::create_random_transaction_id().to_be_bytes());

        // -1 means the tracker chooses the number of peers to return.
        request[92..96].copy_from_slice(&(-1i32).to_be_bytes());
        request[96..98].copy_from_slice(&port.to_be_bytes());

        let sent = self.socket.send_to(&request, self.tracker_addr).await?;
        if sent != request.len() {
            return Err(GeneralError("Failed to send the entire ANNOUNCE request"));
        }

        let mut buffer = [0u8; 65536];
        let (read, source) =
            time::timeout(self.timeout, self.socket.recv_from(&mut buffer)).await??;

        if source != *self.tracker_addr {
            return Err(ApplicationError(
                "ANNOUNCE response came from an unexpected address",
            ));
        }

        if read < 8 {
            return Err(ApplicationError("Incomplete ANNOUNCE response"));
        }

        let action = u32::from_be_bytes(buffer[0..4].try_into().unwrap());
        let response_transaction_id =
            u32::from_be_bytes(buffer[4..8].try_into().unwrap());

        if response_transaction_id != transaction_id {
            return Err(ApplicationError(
                "ANNOUNCE response has an unexpected transaction ID",
            ));
        }

        match action {
            ACTION_ANNOUNCE => {
                if read < 20 {
                    return Err(ApplicationError("Incomplete ANNOUNCE response"));
                }

                let interval =
                    i32::from_be_bytes(buffer[8..12].try_into().unwrap());
                let leechers =
                    i32::from_be_bytes(buffer[12..16].try_into().unwrap());
                let seeders =
                    i32::from_be_bytes(buffer[16..20].try_into().unwrap());

                let peers = match self.tracker_addr {
                    SocketAddr::V4(_) => parse_ipv4_peers(&buffer[20..read])?,
                    SocketAddr::V6(_) => parse_ipv6_peers(&buffer[20..read])?,
                };

                Ok(AnnounceResponse {
                    interval,
                    leechers,
                    seeders,
                    peers,
                })
            }

            ACTION_ERROR => Err(ApplicationError(
                "Tracker returned an error to ANNOUNCE",
            )),

            _ => Err(ApplicationError(
                "Expected ANNOUNCE response, got another response type",
            )),
        }
    }

    fn create_random_transaction_id() -> u32 {
        let mut hasher = DefaultHasher::default();
        SystemTime::now().hash(&mut hasher);
        hasher.finish() as u32
    }
}

#[derive(Clone, Copy)]
pub enum AnnounceEvent {
    None = EVENT_NONE as isize,
    Started = EVENT_STARTED as isize,
    Stopped = EVENT_STOPPED as isize,
}

fn parse_ipv4_peers(data: &[u8]) -> UdpTrackerClientResult<Vec<SocketAddr>> {
    if data.len() % 6 != 0 {
        return Err(ApplicationError(
            "Invalid IPv4 peer list in ANNOUNCE response",
        ));
    }

    Ok(data
        .chunks_exact(6)
        .map(|peer| {
            let ip = Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]);
            let port = u16::from_be_bytes([peer[4], peer[5]]);
            SocketAddr::new(IpAddr::V4(ip), port)
        })
        .collect())
}

fn parse_ipv6_peers(data: &[u8]) -> UdpTrackerClientResult<Vec<SocketAddr>> {
    if data.len() % 18 != 0 {
        return Err(ApplicationError(
            "Invalid IPv6 peer list in ANNOUNCE response",
        ));
    }

    Ok(data
        .chunks_exact(18)
        .map(|peer| {
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&peer[0..16]);

            let port = u16::from_be_bytes([peer[16], peer[17]]);

            SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip)), port)
        })
        .collect())
}

pub type UdpTrackerClientResult<T> = Result<T, UdpTrackerClientError>;

#[derive(Debug)]
pub enum UdpTrackerClientError {
    GeneralError(&'static str),
    IoError(io::Error),
    ApplicationError(&'static str),
}

impl From<io::Error> for UdpTrackerClientError {
    fn from(err: Error) -> Self {
        UdpTrackerClientError::IoError(err)
    }
}

impl From<Elapsed> for UdpTrackerClientError {
    fn from(_: Elapsed) -> Self {
        UdpTrackerClientError::IoError(io::Error::new(TimedOut, ""))
    }
}
