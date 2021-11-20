use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use tokio::io;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TransportType {
    UDP,
    HTTP,
    HTTPS
}

impl TransportType {
    fn to_string(&self) -> &'static str {
        match self {
            Self::UDP => "udp",
            Self::HTTP => "http",
            Self::HTTPS => "https"
        }
    }

    fn from_string(s: &str) -> Result<TransportType, &'static str> {
        match s {
            "udp" => Ok(Self::UDP),
            "http" => Ok(Self::HTTP),
            "https" => Ok(Self::HTTPS),
            _ => Err("Illegal protocol")
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TrackerCandidate {
    pub host: String,
    pub port: u16,
    pub transport_type: TransportType,
    pub suffix: Option<String>
}

impl PartialOrd<Self> for TrackerCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.to_string().partial_cmp(&other.to_string())
    }
}

impl Ord for TrackerCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl TrackerCandidate {
    pub fn to_string(&self) -> String {
        return format!("{}://{}:{}{}",
            self.transport_type.to_string(), self.host, self.port, self.suffix.as_ref().unwrap_or(&String::from(""))
        );
    }

    pub fn from_string(string: &str) -> Result<TrackerCandidate, &'static str> {
        let parts = string.split(':').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err("Invalid format. Expecting two ':'");
        }
        let transport_type = TransportType::from_string(parts[0])?;
        if !parts[1].starts_with("//") {
            return Err("Invalid format. Expecting proto://host:port[/suffix]. Missing '://' after proto");
        }
        let (_, host_str) = parts[1].split_at(2);
        let host = String::from(host_str);
        let suffix_index = parts[2].find('/');
        let port;
        let mut suffix = None;
        if suffix_index.is_some() {
            let (port_str, suffix_str) = parts[2].split_at(suffix_index.unwrap());
            port = port_str.parse().map_err(|_| "Expected port to be a numeric value")?;
            suffix = Some(String::from(suffix_str));
        } else {
            port = parts[2].parse().map_err(|_| "Expected port to be a numeric value")?;
        }
        return Ok(TrackerCandidate {
            host: String::from(host),
            port,
            transport_type,
            suffix
        })
    }
}

pub async fn clean_candidates(file_path: &str) -> io::Result<()> {
    let candidates = get_candidates(file_path).await?;
    println!("Loaded candidates: {}", candidates.len());
    let mut candidates = remove_duplicates(candidates);
    candidates.sort();
    println!("Unique candidates: {}", candidates.len());
    let s = candidates.into_iter()
        .map(|candidate| candidate.to_string())
        .reduce(|a, b| format!("{}\n{}", a, b))
        .unwrap_or(String::from(""));
    tokio::fs::write(file_path, s).await
}

pub async fn get_candidates(file_path: &str) -> io::Result<Vec<TrackerCandidate>> {
    Ok(tokio::fs::read_to_string(file_path).await?
        .split('\n')
        .map(|s| s.trim())
        .filter(|s| !s.starts_with('#'))
        .filter_map(|string| TrackerCandidate::from_string(string).ok())
        .collect::<Vec<_>>())
}

fn remove_duplicates(candidates: Vec<TrackerCandidate>) -> Vec<TrackerCandidate> {
    let mut set = HashSet::with_capacity(candidates.len());
    candidates.into_iter().for_each(|candidate| {
        set.insert(candidate);
    });
    set.into_iter().collect()
}