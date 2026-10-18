//! Pure Rust SSH transport built on russh + russh-sftp.
//!
//! Supports: private key file (`key_path`, or ~/.ssh/id_ed25519 / id_rsa /
//! id_ecdsa by default), password auth (`host.password`), and accepts any
//! server host key.

use crate::hosts::Host;
use crate::transport::{Output, RemoteProcess};
use anyhow::{Result, anyhow, bail};
use russh::client::{self, Handle};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::load_secret_key;
use russh::{Channel, ChannelMsg, Disconnect};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub struct RusshTransport {
    pub key_path: Option<String>,
}

struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // accept any host key (mirrors `-o StrictHostKeyChecking=accept-new`
        // of the system transport; benchmark tooling favours convenience)
        Ok(true)
    }
}

async fn connect(host: &Host, key_path: &Option<String>) -> Result<Handle<ClientHandler>> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(3600)),
        ..Default::default()
    });
    let mut handle = client::connect(config, (host.address.as_str(), host.port), ClientHandler)
        .await
        .map_err(|e| anyhow!("connect to {}:{} failed: {}", host.address, host.port, e))?;

    // password auth when configured; a transport-level failure falls through
    // to publickey instead of aborting (some servers reject the method)
    if !host.password.is_empty() {
        match handle
            .authenticate_password(host.user.clone(), host.password.clone())
            .await
        {
            Ok(auth) if auth.success() => return Ok(handle),
            Ok(_) => {}
            Err(e) => eprintln!(
                "warning: password auth to {} failed: {}; trying public keys",
                host.address, e
            ),
        }
    }

    // publickey auth: explicit key, or default locations
    let mut candidates: Vec<String> = Vec::new();
    if let Some(k) = key_path {
        candidates.push(k.clone());
    } else if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = Path::new(&home);
        for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
            candidates.push(home.join(".ssh").join(name).to_string_lossy().to_string());
        }
    }
    for key_file in &candidates {
        let Ok(key_pair) = load_secret_key(key_file, None) else {
            continue;
        };
        let hash_alg = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
        let key = PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash_alg);
        let auth = handle.authenticate_publickey(host.user.clone(), key).await;
        if let Ok(res) = auth {
            if res.success() {
                return Ok(handle);
            }
        }
    }

    handle.disconnect(Disconnect::ByApplication, "auth failed", "English").await.ok();
    bail!("authentication to {} failed (tried password + keys {:?})", host.address, candidates)
}

impl RusshTransport {
    pub async fn exec(&self, host: &Host, cmd: &str) -> Result<Output> {
        let handle = connect(host, &self.key_path).await?;
        let channel = handle.channel_open_session().await?;
        channel.exec(true, cmd).await?;
        let (stdout, code) = run_channel_to_end(channel).await;
        handle.disconnect(Disconnect::ByApplication, "done", "English").await.ok();
        Ok(Output {
            success: code == 0,
            stdout,
            stderr: String::new(),
        })
    }

    pub async fn upload(&self, host: &Host, local: &Path, remote: &str) -> Result<()> {
        let handle = connect(host, &self.key_path).await?;
        let channel = handle.channel_open_session().await?;
        // The SFTP subsystem must be requested before sending SFTP packets;
        // otherwise the INIT packet is never answered and russh-sftp times out.
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| anyhow!("request sftp subsystem on {} failed: {}", host.address, e))?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;

        let data = tokio::fs::read(local)
            .await
            .map_err(|e| anyhow!("read local file {:?} failed: {}", local, e))?;

        use tokio::io::AsyncWriteExt;
        let mut file = sftp
            .open_with_flags(
                remote,
                russh_sftp::protocol::OpenFlags::WRITE | russh_sftp::protocol::OpenFlags::CREATE | russh_sftp::protocol::OpenFlags::TRUNCATE,
            )
            .await?;
        file.write_all(&data).await.map_err(|e| anyhow!("sftp write failed: {}", e))?;
        file.flush().await.map_err(|e| anyhow!("sftp flush failed: {}", e))?;
        file.shutdown().await.ok();
        drop(file);
        sftp.close().await.ok();
        handle.disconnect(Disconnect::ByApplication, "done", "English").await.ok();
        Ok(())
    }

    pub async fn spawn(&self, host: &Host, cmd: &str) -> Result<RemoteProcess> {
        let handle = connect(host, &self.key_path).await?;
        let channel = handle.channel_open_session().await?;
        channel.exec(true, cmd).await?;
        Ok(RemoteProcess::Russh(RusshProcess {
            channel,
            collected: String::new(),
            pending: String::new(),
            eof: false,
        }))
    }
}

async fn run_channel_to_end(mut channel: Channel<client::Msg>) -> (String, u32) {
    let mut stdout = String::new();
    let mut code = 0u32;
    loop {
        let Some(msg) = channel.wait().await else { break };
        match msg {
            ChannelMsg::Data { ref data } => {
                stdout.push_str(&String::from_utf8_lossy(data));
            }
            ChannelMsg::ExtendedData { ref data, .. } => {
                stdout.push_str(&String::from_utf8_lossy(data));
            }
            ChannelMsg::ExitStatus { exit_status } => {
                code = exit_status;
            }
            _ => {}
        }
    }
    (stdout, code)
}

pub struct RusshProcess {
    channel: Channel<client::Msg>,
    /// All complete lines seen so far (used as the final `wait` output).
    collected: String,
    /// Bytes of a line not yet terminated by '\n'.
    pending: String,
    eof: bool,
}

impl RusshProcess {
    fn take_buffered_line(&mut self) -> Option<String> {
        let pos = self.pending.find('\n')?;
        let mut line: String = self.pending.drain(..=pos).collect();
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        self.collected.push_str(&line);
        self.collected.push('\n');
        Some(line)
    }

    /// Next complete stdout line. A line split across several SSH data packets
    /// is buffered until its newline arrives (a JSON result line must not be
    /// returned in fragments).
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            if let Some(line) = self.take_buffered_line() {
                return Some(line);
            }
            if self.eof {
                if self.pending.is_empty() {
                    return None;
                }
                let line = std::mem::take(&mut self.pending);
                self.collected.push_str(&line);
                self.collected.push('\n');
                return Some(line);
            }
            match self.channel.wait().await {
                Some(ChannelMsg::Data { ref data }) => {
                    self.pending.push_str(&String::from_utf8_lossy(data));
                }
                Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                    self.pending.push_str(&String::from_utf8_lossy(data));
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    self.eof = true;
                }
                _ => {}
            }
        }
    }

    pub async fn wait(&mut self) -> Output {
        while self.next_line().await.is_some() {}
        Output {
            success: true,
            stdout: std::mem::take(&mut self.collected),
            stderr: String::new(),
        }
    }
}
