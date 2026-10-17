use enum_delegate;
use redis::aio::ConnectionLike;
use redis::aio::MultiplexedConnection;
use redis::cluster_async::ClusterConnection;
use redis::{Cmd, RedisFuture, Value};
use std::fmt::{Display, Formatter};
use urlencoding::encode;

#[derive(Clone)]
pub struct ClientConfig {
    pub cluster: bool,
    pub address: String,
    pub username: String,
    pub password: String,
    pub tls: bool,
    pub timeout: u64,
}

impl ClientConfig {
    pub async fn get_client(&self) -> Client {
        let username = encode(&self.username);
        let password = encode(&self.password);
        let conn_str = if self.tls {
            format!("rediss://{}:{}@{}/#insecure", username, password, &self.address)
        } else {
            format!("redis://{}:{}@{}", username, password, &self.address)
        };

        if self.cluster {
            let nodes = vec![conn_str];
            let client = redis::cluster::ClusterClient::builder(nodes).connection_timeout(std::time::Duration::from_secs(self.timeout)).build().unwrap();
            let conn = match client.get_async_connection().await {
                Ok(conn) => conn,
                Err(e) => {
                    eprintln!("Failed to connect to {}: {}", self.address, e);
                    std::process::exit(1);
                }
            };
            Client::new(conn.into())
        } else {
            let client = redis::Client::open(conn_str).unwrap();
            let conn = match client.get_multiplexed_async_connection().await {
                Ok(conn) => conn,
                Err(e) => {
                    eprintln!("Failed to connect to {}: {}", self.address, e);
                    std::process::exit(1);
                }
            };
            Client::new(conn.into())
        }
    }

    /// Discover cluster topology via CLUSTER SLOTS on the seed node.
    /// Returns (master_addresses, replica_count, slots_covered).
    pub async fn discover_cluster_topology(&self) -> Option<(Vec<String>, usize, u64)> {
        if !self.cluster {
            return None;
        }
        let username = encode(&self.username);
        let password = encode(&self.password);
        let conn_str = format!("redis://{}:{}@{}", username, password, &self.address);
        let client = match redis::Client::open(conn_str) {
            Ok(c) => c,
            Err(_) => return None,
        };
        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(_) => return None,
        };
        // CLUSTER SLOTS returns [[start, end, [ip, port, id], [ip, port, id]...], ...]
        let slots: Vec<Vec<redis::Value>> = match redis::cmd("CLUSTER").arg("SLOTS").query_async(&mut conn).await {
            Ok(v) => v,
            Err(_) => return None,
        };
        let mut masters: Vec<String> = Vec::new();
        let mut replicas = 0usize;
        let mut covered: u64 = 0;
        for slot_range in slots {
            // first two entries are slot boundaries, remaining are node tuples
            let mut nodes = slot_range.into_iter();
            let start = match nodes.next() {
                Some(redis::Value::Int(v)) => v,
                _ => continue,
            };
            let end = match nodes.next() {
                Some(redis::Value::Int(v)) => v,
                _ => continue,
            };
            covered += (end - start + 1).max(0) as u64;
            for (i, node) in nodes.enumerate() {
                if let redis::Value::Array(parts) = node {
                    if parts.len() >= 2 {
                        if let (redis::Value::BulkString(ip), redis::Value::Int(port)) = (&parts[0], &parts[1]) {
                            let addr = format!("{}:{}", String::from_utf8_lossy(ip), port);
                            if i == 0 {
                                masters.push(addr);
                            } else {
                                replicas += 1;
                            }
                        }
                    }
                }
            }
        }
        Some((masters, replicas, covered))
    }
}

impl Display for ClientConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "RedisConfig {{ cluster: {}, address: {}, username: {}, password: {}, tls: {} }}", self.cluster, self.address, self.username, self.password, self.tls)
    }
}

#[enum_delegate::implement(ConnectionLike,
trait ConnectionLike {
fn req_packed_command < 'a > (& 'a mut self, cmd: & 'a Cmd) -> RedisFuture < 'a, Value >;
fn req_packed_commands < 'a > (
& 'a mut self,
cmd: & 'a redis::Pipeline,
offset: usize,
count: usize,
) -> RedisFuture < 'a, Vec < Value >>;
fn get_db(& self) -> i64;
}
)]
#[derive(Clone)]
enum ClientConnection {
    Standalone(MultiplexedConnection),
    Cluster(ClusterConnection),
}

pub struct Client {
    conn: ClientConnection,
}

impl Client {
    fn new(conn: ClientConnection) -> Client {
        Client { conn }
    }

    pub async fn run_commands(&mut self, cmds: Vec<redis::Cmd>) -> bool {
        match &mut self.conn {
            // Cluster mode: a single pipeline batch may contain keys hashing to
            // different slots, which Redis Cluster rejects (CrossSlot). Send each
            // command individually instead: the cluster client routes each one to
            // its slot's master and the requests are still in flight concurrently,
            // so throughput behaviour matches pipelining while spreading load
            // evenly across all masters.
            ClientConnection::Cluster(_) => {
                // fire all commands concurrently: the cluster client routes each
                // to its slot's master, giving pipelining-like throughput while
                // keeping load evenly spread across masters.
                // ClusterConnection is a cheap handle (clone shares the internal
                // routing state), so cloning per command is the intended pattern
                // to obtain multiple in-flight futures.
                let mut conns = Vec::with_capacity(cmds.len());
                for _ in 0..cmds.len() {
                    conns.push(self.conn.clone());
                }
                let futs: Vec<_> = conns
                    .iter_mut()
                    .zip(cmds.iter())
                    .map(|(conn, cmd)| conn.req_packed_command(cmd))
                    .collect();
                let results = futures::future::join_all(futs).await;
                for r in results {
                    if let Err(e) = r {
                        let error_msg = format!("{:?}", e);
                        if error_msg.contains("THROTTLED: insufficient capacity unit") {
                            return false;
                        } else {
                            eprintln!("Failed to execute command: {:?}", e);
                            std::process::exit(1);
                        }
                    }
                }
                true
            }
            // Standalone mode: use a real pipeline batch.
            ClientConnection::Standalone(_) => {
                let mut pipeline = redis::pipe();
                for cmd in cmds {
                    pipeline.add_command(cmd).ignore();
                }
                match pipeline.query_async(&mut self.conn).await {
                    Ok(()) => true,
                    Err(e) => {
                        let error_msg = format!("{:?}", e);
                        if error_msg.contains("THROTTLED: insufficient capacity unit") {
                            // ignore throttled requests - don't count them in statistics
                            false
                        } else {
                            eprintln!("Failed to execute pipeline: {:?}", e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
    }
}
