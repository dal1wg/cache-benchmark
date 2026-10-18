use cache_bench_ctl::{WorkerResult, hosts::{Host, load_hosts}, report::print_report, shell_escape, transport::{Transport, build_transport}};
use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgAction, Command};
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

async fn detect_arch(transport: &Transport, host: &Host) -> Result<String> {
    if !host.arch.is_empty() {
        return Ok(host.arch.clone());
    }
    let out = transport.exec(host, "uname -m").await?;
    if !out.success {
        bail!("failed to detect architecture on {}: {}", host.address, out.stderr.trim());
    }
    Ok(out.stdout.trim().to_string())
}

async fn distribute(transport: &Transport, hosts: &[Host], bin_dir: &Path, worker: Option<&Path>) -> Result<()> {
    let mut jobs = Vec::new();
    for host in hosts {
        let transport = transport_ref(transport);
        let host = host.clone();
        let bin_dir = bin_dir.to_path_buf();
        let worker = worker.map(|p| p.to_path_buf());
        jobs.push(tokio::spawn(async move {
            let tag = format!("{}:{}", host.user, host.address);
            let arch = detect_arch(&transport, &host).await?;
            let binary = match &worker {
                Some(w) => w.clone(),
                None => cache_bench_ctl::find_worker_binary(&bin_dir, &arch)?,
            };
            transport.exec(&host, &format!("mkdir -p {}", shell_escape(&host.remote_dir))).await?;
            let remote_path = format!("{}/cache-benchmark", host.remote_dir);
            let size = std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0);
            transport.upload(&host, &binary, &remote_path).await?;
            transport.exec(&host, &format!("chmod +x {}", shell_escape(&remote_path))).await?;
            println!("[{}] distributed {:?} ({:.1} MB, arch {})", tag.cyan(), binary, size as f64 / 1e6, arch);
            Ok::<(), anyhow::Error>(())
        }));
    }
    let mut failed = 0;
    for job in jobs {
        if let Err(e) = job.await.unwrap() {
            eprintln!("{}", format!("distribute failed: {}", e).red());
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{} host(s) failed to distribute", failed);
    }
    Ok(())
}

fn transport_ref(t: &Transport) -> Transport {
    match t {
        Transport::System => Transport::System,
        #[cfg(feature = "russh")]
        Transport::Russh(r) => Transport::Russh(cache_bench_ctl::russh_transport::RusshTransport { key_path: r.key_path.clone() }),
    }
}

struct RunArgs {
    worker_args: Vec<String>,
    hosts: Vec<Host>,
    transport: Transport,
    bin_dir: PathBuf,
    worker: Option<PathBuf>,
    skip_distribute: bool,
    lead_secs: u64,
    output: Option<String>,
}

async fn run(args: RunArgs) -> Result<()> {
    if !args.skip_distribute {
        distribute(&args.transport, &args.hosts, &args.bin_dir, args.worker.as_deref()).await?;
    }

    // synchronized start timestamp
    let start_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + args.lead_secs.max(1);
    println!("launching {} host(s) in parallel, synchronized start at unix {} ...", args.hosts.len(), start_at);

    let mut jobs = Vec::new();
    for host in &args.hosts {
        let transport = transport_ref(&args.transport);
        let host = host.clone();
        let worker_args = args.worker_args.clone();
        jobs.push(tokio::spawn(async move {
            println!("[{}:{}] ssh connected, launching worker (synchronized start at {})", host.user.cyan(), host.address.cyan(), start_at);
            let r = run_on_host(&transport, &host, &worker_args, start_at).await;
            if let Err(e) = &r {
                eprintln!("[{}:{}] {}", host.user, host.address, format!("failed: {:#}", e).red());
            }
            r
        }));
    }

    let mut results: Vec<(Host, Result<WorkerResult>)> = Vec::new();
    for (job, host) in jobs.into_iter().zip(args.hosts.iter()) {
        results.push((host.clone(), job.await.unwrap()));
    }

    print_report(&results, &args.output)?;
    Ok(())
}

async fn run_on_host(transport: &Transport, host: &Host, worker_args: &[String], start_at: u64) -> Result<WorkerResult> {
    let tag = format!("{}:{}", host.user, host.address);
    let remote_path = format!("{}/cache-benchmark", host.remote_dir);

    // Build the remote command: worker args + synchronized start + json output
    let mut cmd = format!("{} ", shell_escape(&remote_path));
    for arg in worker_args {
        cmd.push_str(&shell_escape(arg));
        cmd.push(' ');
    }
    cmd.push_str(&format!("--start-at {} -q --json", start_at));

    let launch_instant = std::time::Instant::now();
    let mut proc = transport.spawn(host, &cmd).await?;
    let mut json_line: Option<String> = None;
    while let Some(line) = proc.next_line().await {
        let line = line.trim().to_string();
        if line.starts_with('{') && line.contains("\"qps\"") {
            json_line = Some(line);
        }
    }
    let output = proc.wait().await;
    if !output.success {
        bail!("worker on {} exited with failure: {}", host.address, output.stderr.trim());
    }
    // fallback: scan the final output in case a line was not surfaced by the
    // streaming reader (e.g. transport-specific chunking)
    let line = json_line
        .or_else(|| {
            output
                .stdout
                .lines()
                .rev()
                .map(str::trim)
                .find(|l| l.starts_with('{') && l.contains("\"qps\""))
                .map(str::to_string)
        })
        .ok_or_else(|| {
            let mut snippet = output.stdout.trim().to_string();
            if snippet.len() > 600 {
                snippet.truncate(600);
                snippet.push_str("...");
            }
            anyhow!(
                "no JSON result line received from {}; remote cmd: {}; output: {}",
                host.address,
                cmd,
                snippet
            )
        })?;
    let result: WorkerResult = serde_json::from_str(&line)
        .map_err(|e| anyhow!("failed to parse result from {}: {} (line: {})", host.address, e, line))?;
    println!("[{}] done: qps {:.0}, p99 {:.2}ms (elapsed {:.1}s)", tag.cyan(), result.qps, result.p99_latency_ms, launch_instant.elapsed().as_secs_f64());
    Ok(result)
}

fn main() -> Result<()> {
    let matches = Command::new("cache-bench-ctl")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Distribute cache-benchmark to multiple servers, run them simultaneously, and collect results")
        .disable_help_subcommand(true)
        .arg(Arg::new("hosts").long("hosts").value_name("FILE").default_value("hosts.toml").global(true).help("Path to the hosts.toml file"))
        .arg(Arg::new("transport").long("transport").value_name("NAME").default_value("ssh").global(true).help("Transport: ssh (system ssh/scp) or russh (pure Rust SSH)"))
        .arg(Arg::new("key").long("key").value_name("PATH").global(true).help("Private key path (russh transport only)"))
        .arg(Arg::new("bin-dir").long("bin-dir").value_name("DIR").default_value("dist").global(true).help("Local directory containing worker binaries named cache-benchmark-<arch> (e.g. cache-benchmark-x86_64)"))
        .arg(Arg::new("binary").long("binary").value_name("FILE").global(true).help("Explicit worker binary path (overrides arch detection)"))
        .subcommand(
            Command::new("distribute")
                .about("Distribute the worker binary to all hosts"),
        )
        .subcommand(
            Command::new("run")
                .about("Distribute (unless skipped) and run the benchmark on all hosts simultaneously, then collect results")
                .arg(Arg::new("skip-distribute").long("skip-distribute").action(ArgAction::SetTrue).help("Assume workers are already distributed"))
                .arg(Arg::new("lead").long("lead").value_name("SECS").default_value("5").help("Seconds to wait before the synchronized start"))
                .arg(Arg::new("output").long("output").value_name("FILE").help("Also write the aggregate report as JSON to this file"))
                .trailing_var_arg(true)
                .arg(Arg::new("worker-args").num_args(..).value_name("ARGS").help("Arguments passed to cache-benchmark verbatim (e.g. -- \"GET {key uniform 100000}\" -s 30 -c 64)")),
        )
        .get_matches();

    let hosts_path = matches.get_one::<String>("hosts").unwrap();
    let hosts = load_hosts(hosts_path)?;
    let transport_name = matches.get_one::<String>("transport").unwrap();
    let transport = build_transport(transport_name, matches.get_one::<String>("key").cloned())?;
    let bin_dir = PathBuf::from(matches.get_one::<String>("bin-dir").unwrap());
    let worker = matches.get_one::<String>("binary").map(PathBuf::from);

    println!("{}: {} host(s), transport: {}", "cache-bench-ctl".bold().blue(), hosts.len(), transport.name());

    match matches.subcommand() {
        Some(("distribute", _)) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(distribute(&transport, &hosts, &bin_dir, worker.as_deref()))
        }
        Some(("run", sub)) => {
            let worker_args: Vec<String> = sub
                .get_many::<String>("worker-args")
                .map(|vals| vals.cloned().collect())
                .unwrap_or_default();
            if worker_args.is_empty() {
                bail!("no worker arguments given; e.g. cache-bench-ctl run -- \"GET {{key uniform 100000}}\" -s 30");
            }
            let args = RunArgs {
                worker_args,
                hosts,
                transport,
                bin_dir,
                worker,
                skip_distribute: *sub.get_one::<bool>("skip-distribute").unwrap(),
                lead_secs: sub.get_one::<String>("lead").unwrap().parse()?,
                output: sub.get_one("output").cloned(),
            };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run(args))
        }
        None => {
            bail!("no subcommand given; use distribute or run (see --help)")
        }
        _ => unreachable!(),
    }
}
