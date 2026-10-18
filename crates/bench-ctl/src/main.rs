use cache_bench_ctl::WorkerResult;
use cache_bench_ctl::hosts::{Host, load_hosts};
use cache_bench_ctl::report::print_report;
use cache_bench_ctl::shell_escape;
use cache_bench_ctl::transport::{Transport, build_transport};
use cache_bench_ctl::{ClockOffset, compensated_start_at, offset_from_probe, parse_remote_timestamp};
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

/// Probe a host's clock offset against the local clock via one SSH exec of
/// `date +%s.%N` (falls back to `+%s` when %N is unsupported). Returns
/// None when both variants fail.
pub async fn probe_clock_offset(transport: &Transport, host: &Host) -> Result<ClockOffset> {
    let mut last_err = String::new();
    for (cmd, precise) in [("date +%s.%N", true), ("date +%s", false)] {
        let t0 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        let out = transport.exec(host, cmd).await?;
        let t1 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        if !out.success {
            last_err = format!("`{}` failed: {}", cmd, out.stderr.trim());
            continue;
        }
        if let Some(remote) = parse_remote_timestamp(&out.stdout) {
            let (offset_secs, rtt_ms) = offset_from_probe(t0, t1, remote);
            return Ok(ClockOffset { offset_secs, rtt_ms, precise });
        }
        last_err = format!("`{}` returned unparsable output: {:?}", cmd, out.stdout);
    }
    bail!("clock probe on {} failed: {}", host.address, last_err)
}

async fn run(args: RunArgs) -> Result<()> {
    if !args.skip_distribute {
        distribute(&args.transport, &args.hosts, &args.bin_dir, args.worker.as_deref()).await?;
    }

    // synchronized start timestamp (reference instant on the LOCAL clock)
    let reference = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64() + args.lead_secs.max(1) as f64;

    // probe each host's clock offset so `--start-at` can be corrected for
    // clock skew (remote = local + offset). Probes run sequentially: each is
    // one quick exec, and serializing them avoids skewing the RTTs.
    let mut offsets: Vec<ClockOffset> = Vec::with_capacity(args.hosts.len());
    for host in &args.hosts {
        match probe_clock_offset(&args.transport, host).await {
            Ok(o) => {
                let note = if o.precise { "" } else { " (second-granularity remote clock)" };
                println!(
                    "clock probe {}: offset {:+.3}s, rtt {:.0}ms{}",
                    host.address, o.offset_secs, o.rtt_ms, note
                );
                offsets.push(o);
            }
            Err(e) => {
                eprintln!("{}", format!("clock probe {} failed: {:#}; assuming offset 0", host.address, e).yellow());
                offsets.push(ClockOffset { offset_secs: 0.0, rtt_ms: 0.0, precise: false });
            }
        }
    }

    // probe time already elapsed; make sure the reference instant is still
    // comfortably in the future on every (possibly skewed) host clock
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    let max_ahead = offsets.iter().map(|o| o.offset_secs).fold(0.0_f64, f64::max);
    let min_headroom = args.lead_secs.max(1) as f64 + max_ahead + 2.0;
    let reference = if reference - now < min_headroom {
        let r = now + min_headroom;
        println!("start reference moved to unix {:.3} to clear clock probes and skew", r);
        r
    } else {
        reference
    };

    println!("launching {} host(s) in parallel, synchronized start at unix {:.3} ...", args.hosts.len(), reference);

    // (host, per-host start timestamp on that host's own clock)
    let launch_plan: Vec<(Host, f64)> = args
        .hosts
        .iter()
        .cloned()
        .zip(offsets.iter().map(|o| compensated_start_at(reference, o.offset_secs)))
        .collect();

    let mut jobs = Vec::new();
    for (host, start_at) in launch_plan {
        let transport = transport_ref(&args.transport);
        let worker_args = args.worker_args.clone();
        jobs.push(tokio::spawn(async move {
            println!(
                "[{}:{}] ssh connected, launching worker (start at {:.3} on its clock)",
                host.user.cyan(), host.address.cyan(), start_at
            );
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

async fn run_on_host(transport: &Transport, host: &Host, worker_args: &[String], start_at: f64) -> Result<WorkerResult> {
    let tag = format!("{}:{}", host.user, host.address);
    let remote_path = format!("{}/cache-benchmark", host.remote_dir);

    // Build the remote command: worker args + synchronized start + json output
    let mut cmd = format!("{} ", shell_escape(&remote_path));
    for arg in worker_args {
        cmd.push_str(&shell_escape(arg));
        cmd.push(' ');
    }
    cmd.push_str(&format!("--start-at {:.3} -q --json", start_at));

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
