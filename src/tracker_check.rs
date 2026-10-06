use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Instant;

use tokio::io;
use tokio::net::lookup_host;

use crate::candidates::TrackerCandidate;
use crate::tracker_client::{
    AnnounceEvent,
    UdpTrackerClient,
    UdpTrackerClientError,
};

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
            }
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

pub async fn check_udp_candidate(
    candidate: TrackerCandidate,
) -> Result<CandidateProfile, CheckError> {
    let addrs = lookup_host(format!("{}:{}", &candidate.host, &candidate.port))
        .await
        .map_err(|_| CheckError::DnsResolutionFailed)?
        .collect::<Vec<_>>();

    if addrs.is_empty() {
        return Err(CheckError::DnsResolutionFailed);
    }

    let responses = addrs
        .iter()
        .map(|address| async move {
            let socket = tokio::net::UdpSocket::bind(match address {
                SocketAddr::V4(_) => "0.0.0.0:0",
                SocketAddr::V6(_) => "[::]:0",
            })
            .await
            .unwrap();

            let mut client = UdpTrackerClient::new(&socket, address);
            let timestamp = Instant::now();

            client.connect().await?;

            /*
             * Preserve the old test values. bip_util padded these byte
             * strings to the 20-byte BitTorrent info-hash/peer-id fields.
             */
            let mut info_hash = [0u8; 20];
            info_hash[..12].copy_from_slice(b"tracker_test");

            let mut peer_id = [0u8; 20];
            peer_id[..7].copy_from_slice(b"tracker");

            let local_port = socket
                .local_addr()
                .expect("Bind to have succeeded");

            let announce_resp = client
                .announce(
                    &info_hash,
                    &peer_id,
                    0,
                    100,
                    0,
                    AnnounceEvent::Started,
                    local_port.port(),
                )
                .await?;

            let rtt = timestamp.elapsed();

            let is_local_peer_returned = announce_resp
                .peers
                .iter()
                .any(|peer| local_port.port() == peer.port());

            if is_local_peer_returned {
                // Clean up after ourselves by removing the announce.
                let _ = client
                    .announce(
                        &info_hash,
                        &peer_id,
                        0,
                        100,
                        0,
                        AnnounceEvent::Stopped,
                        local_port.port(),
                    )
                    .await;

                Ok((address, rtt))
            } else {
                Err(CheckError::OperationalError)
            }
        })
        .collect::<Vec<_>>();

    let responses = futures::future::join_all(responses).await;

    let ok_count = responses
        .iter()
        .filter(|response| response.is_ok())
        .count();

    if ok_count == responses.len() {
        let rtt_ms = responses
            .iter()
            .filter_map(|response| response.as_ref().ok())
            .map(|response| response.1)
            .map(|duration| duration.as_millis() as u32)
            .sum::<u32>()
            / responses.len() as u32;

        return Ok(CandidateProfile {
            candidate,
            addrs,
            rtt_ms,
        });
    }

    let op_errors = responses
        .iter()
        .filter_map(|response| response.as_ref().err())
        .filter(|err| **err == CheckError::OperationalError)
        .count();

    if op_errors > 0 {
        return Err(CheckError::OperationalError);
    }

    let timeouts = responses
        .iter()
        .filter_map(|response| response.as_ref().err())
        .filter(|err| **err == CheckError::Timeout)
        .count();

    if timeouts < responses.len() {
        return Err(CheckError::PartialTimeout);
    }

    Err(CheckError::Timeout)
}
