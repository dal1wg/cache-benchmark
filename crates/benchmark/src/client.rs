use enum_delegate;
use redis::aio::ConnectionLike;
use redis::aio::MultiplexedConnection;
use redis::cluster_async::ClusterConnection;
use redis::{Cmd, RedisFuture, Value};
use std::fmt::{Display, Formatter};
use urlencoding::encode;
use std::sync::Arc;
pub use crate::cluster_pipeline::ClusterLayout;
use crate::cluster_pipeline::{ClusterPipeline, run_routed};

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
    fn connection_url(&self, address: &str) -> String {
        let scheme = if self.tls { "rediss" } else { "redis" };
        let suffix = if self.tls { "/#insecure" } else { "" };
        format!("{scheme}://{}:{}@{address}{suffix}", encode(&self.username), encode(&self.password))
    }

    pub(crate) async fn standalone_connection(&self, address: &str) -> redis::RedisResult<MultiplexedConnection> {
        let client = redis::Client::open(self.connection_url(address))?;
        tokio::time::timeout(std::time::Duration::from_secs(self.timeout), client.get_multiplexed_async_connection()).await
            .map_err(|_| redis::RedisError::from((redis::ErrorKind::IoError, "connection timed out")))?
    }

    pub(crate) async fn cluster_connection(&self) -> redis::RedisResult<ClusterConnection> {
        redis::cluster::ClusterClient::builder(vec![self.connection_url(&self.address)])
            .connection_timeout(std::time::Duration::from_secs(self.timeout)).build()?
            .get_async_connection().await
    }

    pub async fn get_client(&self) -> Client {
        self.get_client_with_layout(None).await
    }

    pub(crate) async fn get_client_with_layout(&self, layout: Option<Arc<ClusterLayout>>) -> Client {
        if self.cluster {
            if let Some(layout) = layout {
                return Client { conn: None, fast: Some(ClusterPipeline::new(self.clone(), layout)) };
            }
        }
        let connection = if self.cluster {
            self.cluster_connection().await.map(ClientConnection::Cluster)
        } else {
            self.standalone_connection(&self.address).await.map(ClientConnection::Standalone)
        };
        match connection {
            Ok(conn) => Client { conn: Some(conn), fast: None },
            Err(error) => {
                eprintln!("Failed to connect to {}: {}", self.address, error);
                std::process::exit(1);
            }
        }
    }

    pub async fn discover_cluster_layout(&self) -> Option<ClusterLayout> {
        if !self.cluster { return None; }
        let mut conn = self.standalone_connection(&self.address).await.ok()?;
        let slots: Vec<Vec<Value>> = redis::cmd("CLUSTER").arg("SLOTS").query_async(&mut conn).await.ok()?;
        ClusterLayout::from_slots(slots, &self.address)
    }

    pub async fn discover_cluster_topology(&self) -> Option<(Vec<String>, usize, u64)> {
        self.discover_cluster_layout().await.map(|layout| (layout.masters, layout.replicas, layout.covered))
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
    conn: Option<ClientConnection>,
    fast: Option<ClusterPipeline>,
}

impl Client {
    pub async fn run_commands(&mut self, cmds: Vec<redis::Cmd>) -> bool {
        let cluster = self.fast.is_some() || matches!(self.conn, Some(ClientConnection::Cluster(_)));
        let result = if let Some(fast) = &mut self.fast {
            fast.run(&cmds).await
        } else {
            match self.conn.as_mut().unwrap() {
                ClientConnection::Cluster(conn) => run_routed(conn, &cmds).await,
                ClientConnection::Standalone(conn) => {
                    let mut pipeline = redis::pipe();
                    for cmd in cmds { pipeline.add_command(cmd).ignore(); }
                    pipeline.query_async::<()>(conn).await
                }
            }
        };
        match result {
            Ok(()) => true,
            Err(error) if error.to_string().contains("THROTTLED: insufficient capacity unit") => false,
            Err(error) => {
                eprintln!("Failed to execute {}: {error}", if cluster { "cluster command" } else { "command" });
                std::process::exit(1);
            }
        }
    }
}
