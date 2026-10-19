//! True per-master pipelines for common single-key commands. Commands needing
//! generic routing retain redis-rs's cluster client. Redirected commands alone
//! are retried through that client; successful writes are never replayed.
use crate::client::ClientConfig;
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster_async::ClusterConnection;
use redis::{Arg, Cmd, ErrorKind, RedisResult, Value};
use std::sync::Arc;

#[derive(Debug)]
pub struct ClusterLayout {
    pub masters: Vec<String>,
    pub replicas: usize,
    pub covered: u64,
    slots: Vec<usize>,
}

impl ClusterLayout {
    pub(crate) fn from_slots(ranges: Vec<Vec<Value>>, seed: &str) -> Option<Self> {
        let mut layout = Self { masters: Vec::new(), replicas: 0, covered: 0, slots: vec![usize::MAX; 16384] };
        for range in ranges {
            let [Value::Int(start), Value::Int(end), Value::Array(master), rest @ ..] = range.as_slice() else { return None };
            if *start < 0 || *end < *start || *end >= 16384 { return None; }
            let [host, Value::Int(port), ..] = master.as_slice() else { return None };
            let host = match host {
                Value::BulkString(bytes) if !bytes.is_empty() => String::from_utf8(bytes.clone()).ok()?,
                Value::BulkString(_) | Value::Nil => seed.rsplit_once(':')?.0.trim_matches(['[', ']']).to_owned(),
                _ => return None,
            };
            let port = u16::try_from(*port).ok()?;
            let address = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
            let node = match layout.masters.iter().position(|m| m == &address) {
                Some(i) => i,
                None => { layout.masters.push(address); layout.masters.len() - 1 }
            };
            for slot in *start as usize..=*end as usize {
                if layout.slots[slot] != usize::MAX { return None; }
                layout.slots[slot] = node;
                layout.covered += 1;
            }
            layout.replicas += rest.len();
        }
        if layout.covered != 16384 { return None; }
        Some(layout)
    }

    fn node_for(&self, cmd: &Cmd) -> Option<usize> {
        let mut args = cmd.args_iter();
        let Arg::Simple(verb) = args.next()? else { return None };
        // Do not guess the key positions of arbitrary commands or Lua scripts.
        if ![b"GET".as_slice(), b"SET", b"INCR", b"INCRBY", b"INCRBYFLOAT", b"DECR", b"DECRBY", b"HGET", b"HSET", b"LPUSH", b"RPUSH", b"SADD", b"ZADD"].iter().any(|v| verb.eq_ignore_ascii_case(v)) {
            return None;
        }
        let Arg::Simple(key) = args.next()? else { return None };
        Some(self.slots[hash_slot(key) as usize])
    }
}

fn hash_slot(key: &[u8]) -> u16 {
    let mut hashed = key;
    if let Some(open) = key.iter().position(|b| *b == b'{') {
        if let Some(close) = key[open + 1..].iter().position(|b| *b == b'}') {
            if close > 0 { hashed = &key[open + 1..open + 1 + close]; }
        }
    }
    crc16::State::<crc16::XMODEM>::calculate(hashed) % 16384
}

pub(crate) async fn run_routed(conn: &ClusterConnection, cmds: &[Cmd]) -> RedisResult<()> {
    let mut connections: Vec<_> = cmds.iter().map(|_| conn.clone()).collect();
    let futures = connections.iter_mut().zip(cmds).map(|(conn, cmd)| conn.req_packed_command(cmd));
    for result in futures::future::join_all(futures).await {
        result?.extract_error()?;
    }
    Ok(())
}

pub(crate) struct ClusterPipeline {
    config: ClientConfig,
    layout: Arc<ClusterLayout>,
    connections: Vec<Option<MultiplexedConnection>>,
    fallback: Option<ClusterConnection>,
}

impl ClusterPipeline {
    pub(crate) fn new(config: ClientConfig, layout: Arc<ClusterLayout>) -> Self {
        let connections = vec![None; layout.masters.len()];
        Self { config, layout, connections, fallback: None }
    }

    async fn fallback(&mut self) -> RedisResult<&mut ClusterConnection> {
        if self.fallback.is_none() {
            self.fallback = Some(self.config.cluster_connection().await?);
            // Release direct connections after topology changes / generic commands.
            self.connections.fill(None);
        }
        Ok(self.fallback.as_mut().unwrap())
    }

    pub(crate) async fn run(&mut self, cmds: &[Cmd]) -> RedisResult<()> {
        if let Some(conn) = &self.fallback {
            return run_routed(conn, cmds).await;
        }
        let Some(routes): Option<Vec<usize>> = cmds.iter().map(|cmd| self.layout.node_for(cmd)).collect() else {
            return run_routed(self.fallback().await?, cmds).await;
        };
        let mut groups: Vec<(redis::Pipeline, Vec<usize>)> = (0..self.connections.len()).map(|_| (redis::pipe(), Vec::new())).collect();
        for (index, node) in routes.into_iter().enumerate() {
            groups[node].0.add_command(cmds[index].clone());
            groups[node].1.push(index);
        }
        let futures = self.connections.iter_mut().zip(groups).enumerate().filter(|(_, (_, (_, indices)))| !indices.is_empty()).map(|(node, (connection, (pipeline, indices)))| {
            let config = &self.config;
            let address = &self.layout.masters[node];
            async move {
                if connection.is_none() {
                    *connection = Some(config.standalone_connection(address).await?);
                }
                // Keep every reply, including errors, so only commands explicitly
                // rejected by the server can be retried after a redirect.
                let values = connection.as_mut().unwrap().req_packed_commands(&pipeline, 0, indices.len()).await?;
                if values.len() != indices.len() {
                    return Err(redis::RedisError::from((ErrorKind::ResponseError, "pipeline response count mismatch")));
                }
                Ok::<_, redis::RedisError>((indices, values))
            }
        });
        let mut retry = Vec::new();
        for result in futures::future::join_all(futures).await {
            let (indices, values) = result?;
            for (index, value) in indices.into_iter().zip(values) {
                if let Err(error) = value.extract_error() {
                    if matches!(error.kind(), ErrorKind::Moved | ErrorKind::Ask | ErrorKind::TryAgain | ErrorKind::ClusterDown | ErrorKind::MasterDown | ErrorKind::ReadOnly) {
                        retry.push(index);
                    } else {
                        return Err(error);
                    }
                }
            }
        }
        if !retry.is_empty() {
            let conn = self.fallback().await?;
            for index in retry {
                conn.req_packed_command(&cmds[index]).await?.extract_error()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hash_tags_match_redis_spec() {
        assert_eq!(hash_slot(b"123456789"), 12739);
        assert_eq!(hash_slot(b"foo{user}one"), hash_slot(b"bar{user}two"));
        assert_eq!(hash_slot(b"{user}"), hash_slot(b"user"));
        assert_ne!(hash_slot(b"{}user"), hash_slot(b"user"));
    }
    #[test]
    fn unsupported_multi_key_commands_do_not_use_fast_path() {
        let layout = ClusterLayout { masters: vec!["localhost:1".into()], replicas: 0, covered: 16384, slots: vec![0;16384] };
        assert_eq!(layout.node_for(redis::cmd("GET").arg("a")), Some(0));
        assert_eq!(layout.node_for(redis::cmd("MGET").arg("a").arg("b")), None);
        assert_eq!(layout.node_for(redis::cmd("EVAL").arg("return 1").arg(0)), None);
    }
}

#[cfg(test)]
mod real_cluster_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires CLUSTER_SEED pointing to a disposable local cluster"]
    async fn redirected_pipeline_does_not_replay_successful_increments() {
        let config = ClientConfig {
            cluster: true,
            address: std::env::var("CLUSTER_SEED").expect("CLUSTER_SEED"),
            username: String::new(), password: String::new(), tls: false, timeout: 5,
        };
        let mut layout = config.discover_cluster_layout().await.unwrap();
        assert!(layout.masters.len() >= 2);
        let prefix = format!("audit_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let mut first = None;
        let mut second = None;
        for i in 0..10000 {
            let key = format!("{prefix}_{i}");
            let node = layout.slots[hash_slot(key.as_bytes()) as usize];
            if first.is_none() { first = Some((key, node)); }
            else if node != first.as_ref().unwrap().1 { second = Some(key); break; }
        }
        let (first, node) = first.unwrap();
        let second = second.unwrap();
        // Simulate a stale slot map: the first INCR succeeds, the second gets
        // MOVED in the very same pipeline. Only the second may be retried.
        layout.slots[hash_slot(second.as_bytes()) as usize] = node;
        let mut fast = ClusterPipeline::new(config.clone(), Arc::new(layout));
        let cmds = vec![redis::cmd("INCR").arg(&first).clone(), redis::cmd("INCR").arg(&second).clone()];
        fast.run(&cmds).await.unwrap();
        assert!(fast.fallback.is_some());
        let mut conn = config.cluster_connection().await.unwrap();
        for key in [&first, &second] {
            let count: i64 = redis::cmd("GET").arg(key).query_async(&mut conn).await.unwrap();
            assert_eq!(count, 1, "successful commands must not be replayed");
        }
        fast.run(&cmds).await.unwrap();
        for key in [&first, &second] {
            let count: i64 = redis::cmd("GET").arg(key).query_async(&mut conn).await.unwrap();
            assert_eq!(count, 2);
            redis::cmd("DEL").arg(key).exec_async(&mut conn).await.unwrap();
        }
    }
}
