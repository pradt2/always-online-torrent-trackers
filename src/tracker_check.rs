use std::error::Error;
use std::io::ErrorKind;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::{Duration, Instant};

use bip_util::bt::{InfoHash, PeerId};
use bip_utracker::announce::{AnnounceEvent, AnnounceRequest, AnnounceResponse, ClientState, DesiredPeers, SourceIP};
use bip_utracker::announce::SourceIP::ImpliedV4;
use bip_utracker::contact::CompactPeers;
use bip_utracker::option::AnnounceOptions;
use bip_utracker::request::{CONNECT_ID_PROTOCOL_ID, RequestType};
use bip_utracker::request::RequestType::{Announce, Connect, Scrape};
use bip_utracker::response::{ResponseType, TrackerResponse};
use bip_utracker::scrape::ScrapeRequest;
use nom::{AsBytes, IResult};
use tokio::io;
use tokio::net::{lookup_host, UdpSocket};

use crate::candidates::TrackerCandidate;
use crate::tracker_client::{UdpTrackerClient, UdpTrackerClientError};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CheckError {
    DnsResolutionFailed,
    OperationalError,
    PartialTimeout,
    Timeout,
}

impl From<io::Error> for CheckError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            ErrorKind::TimedOut => CheckError::Timeout,
            _ => {
                println!("Io Error {:?}", err);
                CheckError::OperationalError
            }
        }
    }
}

impl From<UdpTrackerClientError> for CheckError {
    fn from(err: UdpTrackerClientError) -> Self {
        match err {
            UdpTrackerClientError::IoError(err) => CheckError::from(err),
            UdpTrackerClientError::ApplicationError(err) => {
                println!("Application error {:?}", err);
                CheckError::OperationalError
            },
            UdpTrackerClientError::GeneralError(err) => {
                println!("General error {:?}", err);
                CheckError::OperationalError
            }
        }
    }
}

#[derive(Debug)]
pub struct CandidateProfile {
    pub candidate: TrackerCandidate,
    pub addrs: Vec<SocketAddr>,
    pub rtt_ms: u32,
}

pub async fn check_udp_candidate(candidate: TrackerCandidate) -> Result<CandidateProfile, CheckError> {
    let addrs = lookup_host(format!("{}:{}", &candidate.host, &candidate.port)).await
        .map_err(|err| CheckError::DnsResolutionFailed)?.collect::<Vec<_>>();
    if addrs.len() == 0 { return Err(CheckError::DnsResolutionFailed); }

    let responses = addrs.iter().map(|address| async move {
        let socket = tokio::net::UdpSocket::bind(match address {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        }.parse::<std::net::SocketAddr>().unwrap()).await.unwrap();

        let mut client = UdpTrackerClient::new(&socket, address);
        let timestamp = Instant::now();
        client.connect().await?;

        let info_hash = InfoHash::from_bytes("tracker_test".as_bytes());
        let peer_id = PeerId::from_bytes("tracker".as_bytes());
        let source_ip = match address {
            SocketAddr::V4(_) => SourceIP::ImpliedV4,
            SocketAddr::V6(_) => SourceIP::ImpliedV6
        };

        let local_port = socket.local_addr().expect("Bind to have succeeded");

        let announce_request = AnnounceRequest::new(
            info_hash,
            peer_id,
            ClientState::new(0, 100, 0, AnnounceEvent::Started),
            source_ip,
            0,
            DesiredPeers::Default,
            local_port.port(),
            AnnounceOptions::new()
        );

        let announce_resp = client.announce(announce_request).await?;

        let rtt = timestamp.elapsed();

        let is_local_peer_returned = announce_resp.peers.iter()
            .find(|peer| local_port.port() == peer.port())
            .is_some();

        if is_local_peer_returned {
            // we clean up after ourselves by removing the announce
            let announce_request = AnnounceRequest::new(
                info_hash,
                peer_id,
                ClientState::new(0, 100, 0, AnnounceEvent::Stopped),
                source_ip,
                0,
                DesiredPeers::Default,
                local_port.port(),
                AnnounceOptions::new()
            );
            client.announce(announce_request).await;
            Ok((address, rtt))
        } else {
            Err(CheckError::OperationalError)
        }
    }).collect::<Vec<_>>();

    let responses = futures::future::join_all(responses).await;

    let ok_count = responses.iter()
        .filter(|response| { response.is_ok() })
        .count();

    if ok_count == responses.len() {
        let rtt_ms = responses.iter()
            .filter_map(|response| response.as_ref().ok())
            .map(|response| response.1)
            .map(|duration| duration.as_millis() as u32)
            .sum::<u32>() / responses.len() as u32;

        return Ok(CandidateProfile {
            candidate,
            addrs,
            rtt_ms,
        });
    }

    let op_errors = responses.iter()
        .filter_map(|response| response.clone().err())
        .filter(|err| err == &CheckError::OperationalError)
        .count();

    if op_errors > 0 {
        return Err(CheckError::OperationalError);
    }

    let timeouts = responses.iter()
        .filter_map(|response| response.clone().err())
        .filter(|err| err == &CheckError::Timeout)
        .count();

    if timeouts < responses.len() {
        return Err(CheckError::PartialTimeout);
    }

    return Err(CheckError::Timeout);
}
