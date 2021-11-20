use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use rand::seq::SliceRandom;
use rand::thread_rng;
use tokio::io;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use crate::candidates::TransportType::UDP;
use crate::tracker_check::CheckError;

mod candidates;
mod tracker_check;
mod tracker_client;

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let stream = candidates::get_candidates("candidates.txt").await?.into_iter();
    let semaphore = Rc::new(Semaphore::new(10));
    let profiles = stream
        .filter(|candidate| candidate.transport_type == UDP)
        .map(|candidate| {
            let semaphore_local_ref = semaphore.clone();
            async move {
                let permit = semaphore_local_ref.acquire().await.expect("Semaphore to be operating");
                let res = tracker_check::check_udp_candidate(candidate.clone()).await;
                drop(permit);
                match &res {
                    Ok(profile) => { println!("Success: {:?}", profile) }
                    Err(err) => { println!("Failure: {:?}", err) }
                }
                res
            }
        })
        .collect::<Vec<_>>();
    let timestamp = Instant::now();
    let profiles = futures::future::join_all(profiles).await;
    let mut all_ok = 0;
    let mut dns_unresolved = 0;
    let mut partial_timeout = 0;
    let mut complete_timeout = 0;
    let mut operational_error = 0;
    profiles.iter().for_each(|res| {
        match res {
            Ok(_) => { all_ok += 1; }
            Err(CheckError::DnsResolutionFailed) => { dns_unresolved += 1; }
            Err(CheckError::PartialTimeout) => { partial_timeout += 1; }
            Err(CheckError::Timeout) => { complete_timeout += 1; }
            Err(CheckError::OperationalError) => { operational_error += 1; }
        }
    });
    println!(
        "OK {} , DNS failure {} , p/Timeout {} , Timeout {} , Operational error {}",
        all_ok, dns_unresolved, partial_timeout, complete_timeout, operational_error
    );

    let mut output_hosts = profiles.iter()
        .filter_map(|res| res.as_ref().ok())
        .map(|profile| profile.candidate.clone())
        .collect::<Vec<_>>();
    output_hosts.shuffle(&mut thread_rng());
    let output_hosts = output_hosts.into_iter()
        .map(|candidate| candidate.to_string())
        .reduce(|a, b| format!("{}\n{}", a, b))
        .unwrap_or(String::from(""));
    tokio::fs::write("udp_hosts.txt", output_hosts).await?;

    let output_ip4 = profiles.iter()
        .filter_map(|res| res.as_ref().ok())
        .flat_map(|profile| profile.addrs.clone().into_iter())
        .filter(|addr| addr.is_ipv4())
        .map(|addr| addr.to_string())
        .collect::<HashSet<_>>();
    let mut output_ip4 = output_ip4.into_iter()
        .collect::<Vec<_>>();
    output_ip4.shuffle(&mut thread_rng());
    let output_ip4 = output_ip4.into_iter()
        .reduce(|a, b| format!("{}\n{}", a, b))
        .unwrap_or(String::from(""));
    tokio::fs::write("udp_ipv4s.txt", output_ip4).await?;

    let output_ip6 = profiles.iter()
        .filter_map(|res| res.as_ref().ok())
        .flat_map(|profile| profile.addrs.clone().into_iter())
        .filter(|addr| addr.is_ipv6())
        .map(|addr| addr.to_string())
        .collect::<HashSet<_>>();
    let mut output_ip6 = output_ip6.into_iter()
        .collect::<Vec<_>>();
    output_ip6.shuffle(&mut thread_rng());
    let output_ip6 = output_ip6.into_iter()
        .reduce(|a, b| format!("{}\n{}", a, b))
        .unwrap_or(String::from(""));
    tokio::fs::write("udp_ipv6s.txt", output_ip6).await?;

    println!("Finished in {:?}", timestamp.elapsed());
    Ok(())
}
