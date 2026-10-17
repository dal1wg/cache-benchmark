use serde::{Deserialize, Serialize};

/// One benchmark host entry in hosts.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub address: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default = "default_user")]
    pub user: String,
    /// Override architecture detection (e.g. "x86_64" or "aarch64").
    #[serde(default)]
    pub arch: String,
    /// Remote directory where the worker binary is placed.
    #[serde(default = "default_remote_dir")]
    pub remote_dir: String,
    /// SSH password. When non-empty, the system ssh transport uses it via
    /// `sshpass` (must be installed on the management machine); the russh
    /// transport supports password auth natively. Empty = key/agent/config.
    #[serde(default)]
    pub password: String,
}

fn default_ssh_port() -> u16 {
    22
}
fn default_user() -> String {
    "root".to_string()
}
fn default_remote_dir() -> String {
    "/tmp/cache-benchmark".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostsFile {
    pub hosts: Vec<Host>,
}

pub fn parse_hosts(content: &str) -> anyhow::Result<Vec<Host>> {
    let file: HostsFile = toml::from_str(content)?;
    if file.hosts.is_empty() {
        anyhow::bail!("no hosts defined in hosts file");
    }
    Ok(file.hosts)
}

pub fn load_hosts(path: &str) -> anyhow::Result<Vec<Host>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read hosts file {}: {}", path, e))?;
    parse_hosts(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hosts() {
        let content = r#"
[[hosts]]
address = "192.168.102.34"
user = "root"
remote_dir = "/root/cache-benchmark/tmp"

[[hosts]]
address = "192.168.102.35"
"#;
        let hosts = parse_hosts(content).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].address, "192.168.102.34");
        assert_eq!(hosts[0].user, "root");
        assert_eq!(hosts[0].port, 22);
        assert_eq!(hosts[0].remote_dir, "/root/cache-benchmark/tmp");
        assert_eq!(hosts[1].address, "192.168.102.35");
        assert_eq!(hosts[1].user, "root"); // default
        assert_eq!(hosts[1].remote_dir, "/tmp/cache-benchmark"); // default
    }

    #[test]
    fn test_parse_hosts_empty_fails() {
        assert!(parse_hosts("other = 1\n").is_err());
    }
}
