use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::Error;
use std::io::ErrorKind::TimedOut;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime};
use bip_util::bt::{InfoHash, PeerId};
use bip_utracker::{announce, contact, request, response};
use bip_utracker::announce::{AnnounceRequest};
use bip_utracker::contact::CompactPeers;
use bip_utracker::request::CONNECT_ID_PROTOCOL_ID;
use bip_utracker::request::RequestType::Connect;
use nom::IResult;
use tokio::net::UdpSocket;
use tokio::time;
use tokio::time::error::Elapsed;
use self::UdpTrackerClientError::{ApplicationError, GeneralError};
use std::convert::TryInto;

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
            timeout: Duration::from_secs(5)
        }
    }

    pub async fn connect(&mut self) -> UdpTrackerClientResult<()> {
        let mut buffer = [0u8; 1024];

        let transaction_id = UdpTrackerClient::create_random_transaction_id();

        request::TrackerRequest::new(
            request::CONNECT_ID_PROTOCOL_ID,
            transaction_id,
            request::RequestType::Connect,
        ).write_bytes(&mut buffer[..]).expect("Buffer has sufficient space for CONNECT request");

        if buffer.len() != self.socket.send_to(&buffer, self.tracker_addr).await? {
            return Err(GeneralError("Failed to send the entire CONNECT request"))
        };

        let read = time::timeout(self.timeout, self.socket.recv(&mut buffer)).await??;
        if read > buffer.len() {
            return Err(GeneralError("Failed to read the entire CONNECT response. Buffer too small?"))
        }

        let response = response::TrackerResponse::from_bytes(&buffer[0..read]);
        let response = match response {
            IResult::Done(_, output) => Ok(output),
            IResult::Incomplete(_) => Err(ApplicationError("Incomplete CONNECT response")),
            IResult::Error(_) => Err(ApplicationError("Unknown CONNECT response error"))
        }?;

        let conn_id = match response.response_type() {
            response::ResponseType::Connect(conn_id) => Ok(*conn_id),
            response::ResponseType::Announce(_) => Err(ApplicationError("Expected CONNECT response, got ANNOUNCE response")),
            response::ResponseType::Scrape(_) => Err(ApplicationError("Expected CONNECT response, got SCRAPE response")),
            response::ResponseType::Error(_) => Err(ApplicationError("Expected CONNECT response, got ERROR response"))
        }?;

        self.conn_id = conn_id;
        Ok(())
    }

    pub async fn announce(&self, announce_req: AnnounceRequest<'_>) -> UdpTrackerClientResult<AnnounceResponse> {
        if self.conn_id == 0 {
            return Err(ApplicationError("You have to run connect first!"));
        }

        let mut buffer = [0u8; 1024];

        let transaction_id = UdpTrackerClient::create_random_transaction_id();

        request::TrackerRequest::new(
            self.conn_id,
            transaction_id,
            request::RequestType::Announce(announce_req),
        ).write_bytes(&mut buffer[..]).expect("Buffer has sufficient space for ANNOUNCE request");

        if buffer.len() != self.socket.send_to(&buffer, self.tracker_addr).await? {
            return Err(GeneralError("Failed to send the entire ANNOUNCE request"))
        };

        let read = time::timeout(self.timeout, self.socket.recv(&mut buffer)).await??;
        if read >= buffer.len() {
            return Err(GeneralError("Failed to read the entire ANNOUNCE response. Buffer too small?"))
        }

        let response = response::TrackerResponse::from_bytes(&buffer[0..read]);
        let response = match response {
            IResult::Done(_, output) => Ok(output),
            IResult::Incomplete(_) => Err(ApplicationError("Incomplete ANNOUNCE response")),
            IResult::Error(_) => Err(ApplicationError("Unknown ANNOUNCE response error"))
        }?;

        let announce_response = match response.response_type() {
            response::ResponseType::Announce(announce_response) => Ok(announce_response),
            response::ResponseType::Connect(_) => Err(ApplicationError("Expected ANNOUNCE response, got CONNECT response")),
            response::ResponseType::Scrape(_) => Err(ApplicationError("Expected ANNOUNCE response, got SCRAPE response")),
            response::ResponseType::Error(_) => Err(ApplicationError("Expected ANNOUNCE response, got ERROR response"))
        }?;

        let peers = announce_response.peers().iter().collect::<Vec<_>>();
        Ok(AnnounceResponse {
            interval: announce_response.interval(),
            leechers: announce_response.leechers(),
            seeders: announce_response.seeders(),
            peers
        })
    }

    /// Maybe worth replacing with the `rand` crate in the future
    /// Since this has zero security implications, it is good enough for now
    fn create_random_transaction_id() -> u32 {
        let mut hasher = DefaultHasher::default();
        SystemTime::now().hash(&mut hasher);
        hasher.finish() as u32
    }

}

pub type UdpTrackerClientResult<T> = Result<T, UdpTrackerClientError>;

#[derive(Debug)]
pub enum UdpTrackerClientError {
    GeneralError(&'static str),
    IoError(io::Error),
    ApplicationError(&'static str)
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