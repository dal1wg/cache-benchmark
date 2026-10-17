use crate::hosts::Host;
use anyhow::{Result, anyhow, bail};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Execution output of a remote command.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// How ctl talks to remote hosts.
///
/// - `System`: shell out to the system `ssh`/`scp` binaries
///   (reuses ~/.ssh/config, keys, jump hosts)
/// - `Russh`: pure Rust SSH, no external binaries
///   (key file / password auth)
pub enum Transport {
    System,
    #[cfg(feature = "russh")]
    Russh(crate::russh_transport::RusshTransport),
}

pub fn build_transport(name: &str, _key_path: Option<String>) -> Result<Transport> {
    match name {
        "ssh" => Ok(Transport::System),
        #[cfg(feature = "russh")]
        "russh" => Ok(Transport::Russh(crate::russh_transport::RusshTransport { key_path: _key_path })),
        #[cfg(not(feature = "russh"))]
        "russh" => bail!("--transport russh requires building cache-bench-ctl with the `russh` feature"),
        other => Err(anyhow!("unknown transport: {} (expected ssh or russh)", other)),
    }
}

impl Transport {
    pub fn name(&self) -> &'static str {
        match self {
            Transport::System => "ssh",
            #[cfg(feature = "russh")]
            Transport::Russh(_) => "russh",
        }
    }

    pub async fn exec(&self, host: &Host, cmd: &str) -> Result<Output> {
        match self {
            Transport::System => system_exec(host, cmd).await,
            #[cfg(feature = "russh")]
            Transport::Russh(t) => t.exec(host, cmd).await,
        }
    }

    pub async fn upload(&self, host: &Host, local: &Path, remote: &str) -> Result<()> {
        match self {
            Transport::System => system_upload(host, local, remote).await,
            #[cfg(feature = "russh")]
            Transport::Russh(t) => t.upload(host, local, remote).await,
        }
    }

    /// Spawn a long-running remote command whose stdout lines can be streamed.
    pub async fn spawn(&self, host: &Host, cmd: &str) -> Result<RemoteProcess> {
        match self {
            Transport::System => system_spawn(host, cmd).await,
            #[cfg(feature = "russh")]
            Transport::Russh(t) => t.spawn(host, cmd).await,
        }
    }
}

/// A running remote process: stdout lines can be streamed live with
/// `next_line`; `wait` collects the remaining output and the exit status.
pub enum RemoteProcess {
    SystemSsh {
        child: tokio::process::Child,
        stdout: Option<BufReader<tokio::process::ChildStdout>>,
        stderr_buf: Arc<Mutex<String>>,
        collected: String,
    },
    #[cfg(feature = "russh")]
    Russh(crate::russh_transport::RusshProcess),
}

impl RemoteProcess {
    /// Next stdout line, or None when the stream ended.
    pub async fn next_line(&mut self) -> Option<String> {
        match self {
            RemoteProcess::SystemSsh { stdout, collected, .. } => {
                let Some(reader) = stdout else { return None };
                let mut buf = String::new();
                match reader.read_line(&mut buf).await {
                    Ok(0) => None,
                    Ok(_) => {
                        let line = buf.trim_end_matches(['\n', '\r']).to_string();
                        collected.push_str(&line);
                        collected.push('\n');
                        Some(line)
                    }
                    Err(_) => None,
                }
            }
            #[cfg(feature = "russh")]
            RemoteProcess::Russh(p) => p.next_line().await,
        }
    }

    /// Wait for the process to finish and return the full output.
    pub async fn wait(&mut self) -> Output {
        match self {
            RemoteProcess::SystemSsh { child, stdout, stderr_buf, collected } => {
                // drain remaining stdout
                if let Some(reader) = stdout.take() {
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        collected.push_str(&line);
                        collected.push('\n');
                    }
                }
                let status = child.wait().await;
                let stderr = stderr_buf.lock().unwrap().clone();
                Output {
                    success: matches!(&status, Ok(s) if s.success()),
                    stdout: collected.clone(),
                    stderr,
                }
            }
            #[cfg(feature = "russh")]
            RemoteProcess::Russh(p) => p.wait().await,
        }
    }
}

// ---------- system ssh / scp implementation ----------

/// Build an ssh (or sshpass-wrapped ssh) command base for a host.
///
/// When `host.password` is set, authentication goes through `sshpass -e`
/// (password passed via the SSHPASS environment variable, never in argv).
/// Without a password, `BatchMode=yes` makes ssh fail fast when no
/// key/agent/config authentication is available.
fn ssh_base(host: &Host) -> Command {
    let use_password = !host.password.is_empty();
    let mut cmd = if use_password {
        let mut c = Command::new("sshpass");
        c.arg("-e");
        c.env("SSHPASS", &host.password);
        c.arg("ssh");
        c
    } else {
        Command::new("ssh")
    };
    cmd.arg("-p").arg(host.port.to_string());
    if use_password {
        // allow exactly one password prompt for sshpass to answer
        cmd.arg("-o").arg("NumberOfPasswordPrompts=1");
    } else {
        // never prompt, fail instead
        cmd.arg("-o").arg("BatchMode=yes");
    }
    cmd.arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg("-o").arg("ConnectTimeout=10")
        .arg(format!("{}@{}", host.user, host.address));
    cmd
}

fn scp_base(host: &Host) -> Command {
    let use_password = !host.password.is_empty();
    let mut cmd = if use_password {
        let mut c = Command::new("sshpass");
        c.arg("-e");
        c.env("SSHPASS", &host.password);
        c.arg("scp");
        c
    } else {
        Command::new("scp")
    };
    cmd.arg("-P").arg(host.port.to_string());
    if use_password {
        cmd.arg("-o").arg("NumberOfPasswordPrompts=1");
    } else {
        cmd.arg("-o").arg("BatchMode=yes");
    }
    cmd.arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg("-o").arg("ConnectTimeout=10");
    cmd
}

/// Map a spawn failure to a helpful error, hinting at sshpass when a password
/// is configured but the sshpass binary is missing.
fn spawn_error(host: &Host, what: &str, e: std::io::Error) -> anyhow::Error {
    if !host.password.is_empty() && e.kind() == std::io::ErrorKind::NotFound {
        anyhow!(
            "failed to run {} for {}: {} (password auth over the system ssh transport requires `sshpass`: install it with `apt install sshpass` or `yum install sshpass`, or use --transport russh)",
            what,
            host.address,
            e
        )
    } else {
        anyhow!("failed to run {} for {}: {}", what, host.address, e)
    }
}

async fn system_exec(host: &Host, cmd: &str) -> Result<Output> {
    let out = ssh_base(host)
        .arg(cmd)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| spawn_error(host, "ssh", e))?;
    Ok(Output {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

async fn system_upload(host: &Host, local: &Path, remote: &str) -> Result<()> {
    let local_str = local.to_string_lossy().to_string();
    let out = scp_base(host)
        .arg(&local_str)
        .arg(format!("{}@{}:{}", host.user, host.address, remote))
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| spawn_error(host, "scp", e))?;
    if !out.status.success() {
        bail!("scp to {} failed: {}", host.address, String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

async fn system_spawn(host: &Host, cmd: &str) -> Result<RemoteProcess> {
    let mut child = ssh_base(host)
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(host, "ssh", e))?;

    // drain stderr in the background so the pipe never fills up
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut b = buf.lock().unwrap();
                b.push_str(&line);
                b.push('\n');
            }
        });
    }

    let stdout = child.stdout.take().map(BufReader::new);
    Ok(RemoteProcess::SystemSsh {
        child,
        stdout,
        stderr_buf,
        collected: String::new(),
    })
}
