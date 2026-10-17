//! Minimal in-process RESP server for tests.
//!
//! - Replies `+OK` to every command by default (configurable per verb).
//! - Answers `PING` with `+PONG`.
//! - Answers `CLUSTER SLOTS` with a configurable topology, enabling
//!   cluster-mode code paths without a real cluster.

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct MockRespConfig {
    /// Reply for PING. Default: PONG.
    pub ping_reply: String,
    /// Master nodes to advertise in CLUSTER SLOTS (address strings).
    pub cluster_masters: Vec<String>,
    /// Replica nodes to advertise in CLUSTER SLOTS.
    pub cluster_replicas: Vec<String>,
    /// Number of commands processed (shared counter, for assertions).
    pub cmd_count: Arc<Mutex<u64>>,
}

impl Default for MockRespConfig {
    fn default() -> Self {
        MockRespConfig {
            ping_reply: "PONG".to_string(),
            cluster_masters: vec![],
            cluster_replicas: vec![],
            cmd_count: Arc::new(Mutex::new(0)),
        }
    }
}

impl MockRespConfig {
    pub fn cluster(mut self, masters: Vec<String>, replicas: Vec<String>) -> Self {
        self.cluster_masters = masters;
        self.cluster_replicas = replicas;
        self
    }
}

/// Handle to a running mock server: queryable even while the accept loop
/// (which owns the listener) keeps running.
#[derive(Clone)]
pub struct MockRespHandle {
    port: u16,
    config: MockRespConfig,
}

impl MockRespHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub async fn cmd_count(&self) -> u64 {
        *self.config.cmd_count.lock().await
    }
}

/// Start a mock RESP server on an ephemeral port. The accept loop keeps
/// running until the process ends; tests use the returned handle.
pub async fn start_mock(config: MockRespConfig) -> anyhow::Result<MockRespHandle> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let handle = MockRespHandle { port, config: config.clone() };
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => break,
            };
            let config = config.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, config).await;
            });
        }
    });
    Ok(handle)
}

async fn handle_conn(stream: TcpStream, config: MockRespConfig) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::with_capacity(4096);
    loop {
        buf.clear();
        // read one line (array header)
        if read_line(&mut reader, &mut buf).await == 0 {
            return Ok(()); // connection closed
        }
        let header = String::from_utf8_lossy(&buf).trim().to_string();
        if !header.starts_with('*') {
            continue;
        }
        let n: usize = match header[1..].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mut first_arg = String::new();
        for i in 0..n {
            buf.clear();
            if read_line(&mut reader, &mut buf).await == 0 {
                return Ok(());
            }
            let bulk_header = String::from_utf8_lossy(&buf).trim().to_string();
            let len: usize = match bulk_header.strip_prefix('$').and_then(|s| s.parse().ok()) {
                Some(v) => v,
                None => continue,
            };
            let mut payload = vec![0u8; len];
            reader.read_exact(&mut payload).await?;
            // trailing \r\n after payload
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).await?;
            if i == 0 {
                first_arg = String::from_utf8_lossy(&payload).to_uppercase();
            }
        }

        {
            let mut c = config.cmd_count.lock().await;
            *c += 1;
        }

        let reply = match first_arg.as_str() {
            "PING" => format!("+{}\r\n", config.ping_reply),
            "CLUSTER" => cluster_slots_reply(&config),
            // reject HELLO so clients fall back to RESP2 instead of expecting
            // a RESP3 map reply (a plain +OK here breaks protocol negotiation)
            "HELLO" => "-ERR unknown command 'HELLO'\r\n".to_string(),
            _ => "+OK\r\n".to_string(),
        };
        writer.write_all(reply.as_bytes()).await?;
    }
}

fn cluster_slots_reply(config: &MockRespConfig) -> String {
    let masters = &config.cluster_masters;
    let replicas = &config.cluster_replicas;
    if masters.is_empty() {
        // single-node "cluster" covering all slots, pointing at the mock itself
        return "*1\r\n*3\r\n:0\r\n:16383\r\n*3\r\n$9\r\n127.0.0.1\r\n:6379\r\n$40\r\n0000000000000000000000000000000000000000\r\n".to_string();
    }
    let slots_per_node = 16384 / masters.len() as i64;
    let mut out = format!("*{}\r\n", masters.len());
    for (i, m) in masters.iter().enumerate() {
        let start = slots_per_node * i as i64;
        let end = if i == masters.len() - 1 { 16383 } else { start + slots_per_node - 1 };
        let (host, port) = split_host_port(m);
        let node_count = if replicas.is_empty() { 1 } else { 2 }; // master (+ one replica)
        let mut range = format!("*{}\r\n:{}\r\n:{}\r\n", node_count + 2, start, end);
        range.push_str(&format!("*3\r\n${}\r\n{}\r\n:{}\r\n$40\r\n{:040}\r\n", host.len(), host, port, i));
        if !replicas.is_empty() {
            let (rhost, rport) = split_host_port(&replicas[i % replicas.len()]);
            range.push_str(&format!("*3\r\n${}\r\n{}\r\n:{}\r\n$40\r\n{:040}\r\n", rhost.len(), rhost, rport, i + 100));
        }
        out.push_str(&range);
    }
    out
}

fn split_host_port(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(6379)),
        None => (addr.to_string(), 6379),
    }
}

/// Read a \r\n-terminated line into buf (excluding CRLF). Returns bytes read
/// (0 = EOF).
async fn read_line<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> usize {
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte).await {
            Ok(0) => return 0,
            Ok(_) => {
                if byte[0] == b'\n' {
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                    return buf.len() + 1;
                }
                buf.push(byte[0]);
            }
            Err(_) => return 0,
        }
    }
}
