use cache_benchmark::{make_generator, parse_cores, run_benchmark, Case};
use clap::{Arg, ArgAction, Command};
use colored::Colorize;
use std::time::{SystemTime, UNIX_EPOCH};

fn default_cores() -> String {
    // all logical cores: "0-N"
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    format!("0-{}", n - 1)
}


pub fn main() {
    let matches = Command::new("cache-benchmark")
        .version(env!("CARGO_PKG_VERSION"))
        .about("A high-performance benchmark tool for Redis, Valkey, Tair and any RESP-compatible database")
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .long("help")
                .action(ArgAction::Help)
                .help("Print help"),
        )
        .arg(Arg::new("host").short('h').value_name("host").default_value("127.0.0.1").help("Server hostname (default 127.0.0.1)"))
        .arg(Arg::new("port").short('p').value_name("port").value_parser(clap::value_parser!(u16)).default_value("6379").help("Server port (default 6379)"))
        .arg(Arg::new("username").short('u').value_name("username").default_value("").help("Used to send ACL style \"AUTH username pass\". Needs -a."))
        .arg(Arg::new("password").short('a').value_name("password").default_value("").help("Password for Redis Auth"))
        .arg(Arg::new("clients").short('c').value_name("clients").value_parser(clap::value_parser!(u64)).default_value("0").help("Number of parallel connections (0 for auto, default: 0)"))
        .arg(Arg::new("cores").long("cores").value_name("cores").help("Comma-separated list of CPU cores to use (e.g. 0,1,2 or 0-3, default all)"))
        .arg(Arg::new("cluster").long("cluster").action(ArgAction::SetTrue).help("Use cluster mode (default false)"))
        .arg(Arg::new("requests").short('n').value_name("requests").value_parser(clap::value_parser!(u64)).default_value("0").help("Total number of requests (default 0), 0 for unlimited."))
        .arg(Arg::new("target").short('t').value_name("target").value_parser(clap::value_parser!(u64)).default_value("0").help("Target number of requests per second, 0 for unlimited, recommended to be multiple of 100."))
        .arg(Arg::new("seconds").short('s').value_name("seconds").value_parser(clap::value_parser!(u64)).default_value("0").help("Total time in seconds (default 0), 0 for unlimited."))
        .arg(Arg::new("pipeline").short('P').value_name("pipeline").value_parser(clap::value_parser!(u64)).default_value("1").help("Pipeline <numreq> requests. Default 1 (no pipeline)."))
        .arg(Arg::new("tls").long("tls").action(ArgAction::SetTrue).help("Use TLS for connection (default false)"))
        .arg(Arg::new("load").long("load").action(ArgAction::SetTrue).help("Only load data to Redis, no benchmark."))
        .arg(Arg::new("short-connection").long("short-connection").action(ArgAction::SetTrue).help("Create a new connection for each command and close it after use."))
        .arg(Arg::new("quiet").short('q').action(ArgAction::SetTrue).help("Quiet mode, do not print progress."))
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue).help("Print the final result as a single-line JSON object (for cache-bench-ctl)."))
        .arg(Arg::new("lua").long("lua").action(ArgAction::SetTrue).help("Use lua script for generating random command"))
        .arg(Arg::new("lua-file").long("lua-file").action(ArgAction::SetTrue).help("Use lua script file for generating random command"))
        .arg(Arg::new("start-at").long("start-at").value_name("unix_ts").value_parser(clap::value_parser!(f64)).help("Delay the benchmark until the given unix timestamp (fractional seconds allowed, used by cache-bench-ctl for synchronized start with clock-offset compensation)."))
        .arg(Arg::new("command").value_name("command").index(1).default_value("SET {key uniform 100000} {value 64}").help("The Redis command to benchmark (default SET {key uniform 100000} {value 64})"))
        .get_matches();

    let host: &String = matches.get_one("host").unwrap();
    let port: u16 = *matches.get_one("port").unwrap();
    let username: &String = matches.get_one("username").unwrap();
    let password: &String = matches.get_one("password").unwrap();
    let connections: u64 = *matches.get_one("clients").unwrap();
    let cores_str = matches.get_one::<String>("cores").cloned().unwrap_or_else(default_cores);
    let cores: Vec<u16> = parse_cores(&cores_str);
    let cluster: bool = *matches.get_one("cluster").unwrap();
    let count: u64 = *matches.get_one("requests").unwrap();
    let target: u64 = *matches.get_one("target").unwrap();
    let seconds: u64 = *matches.get_one("seconds").unwrap();
    let pipeline: u64 = *matches.get_one("pipeline").unwrap();
    let tls: bool = *matches.get_one("tls").unwrap();
    let load: bool = *matches.get_one("load").unwrap();
    let short_connection: bool = *matches.get_one("short-connection").unwrap();
    let quiet: bool = *matches.get_one("quiet").unwrap() || *matches.get_one("json").unwrap();
    let json: bool = *matches.get_one("json").unwrap();
    let lua: bool = *matches.get_one("lua").unwrap() || *matches.get_one("lua-file").unwrap();
    let start_at: Option<f64> = matches.get_one("start-at").copied();
    let mut command: String = matches.get_one::<String>("command").unwrap().clone();

    if *matches.get_one::<bool>("lua-file").unwrap() {
        command = std::fs::read_to_string(&command).unwrap_or_else(|e| {
            eprintln!("Failed to read lua file {}: {}", command, e);
            std::process::exit(1);
        });
    }

    if short_connection {
        if load {
            eprintln!("Error: short_connection mode does not support loading");
            std::process::exit(1);
        }
        if pipeline > 1 {
            eprintln!("Error: short_connection mode does not support pipeline (pipeline must be 1)");
            std::process::exit(1);
        }
    }

    // Wait until start-at timestamp (synchronized start for distributed runs).
    // Fractional seconds supported: cache-bench-ctl compensates per-host clock
    // offsets, and sleeping with ms precision keeps the residual error small.
    if let Some(ts) = start_at {
        let deadline = UNIX_EPOCH + std::time::Duration::from_secs_f64(ts.max(0.0_f64));
        match deadline.duration_since(SystemTime::now()) {
            Ok(wait) => {
                if !quiet {
                    println!("waiting {:.2}s for synchronized start at {}", wait.as_secs_f64(), ts);
                }
                // coarse sleep, then spin the last 2ms for ms-level accuracy
                let coarse = wait.saturating_sub(std::time::Duration::from_millis(2));
                std::thread::sleep(coarse);
                while SystemTime::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
            }
            Err(_) => {
                // deadline already passed: start immediately
            }
        }
    }

    // Windows: enable VT processing so ANSI escape codes in progress output work
    enable_ansi_support();

    let client_config = cache_benchmark::client::ClientConfig {
        cluster,
        address: format!("{}:{}", host, port),
        username: username.clone(),
        password: password.clone(),
        tls,
        timeout: 30,
    };
    let case = Case {
        command: make_generator(&command, lua),
        connections,
        pipeline,
        count,
        target,
        seconds,
        short_connection,
    };
    let result = run_benchmark(client_config, cores, case, load, quiet);

    if json {
        println!("{}", serde_json::to_string(&result).unwrap());
    } else {
        println!("{}: {}", "qps".bold().blue(), format!("{:.0}", result.qps));
        println!("{}: {}", "avg".bold().blue(), format!("{:.2}ms", result.avg_latency_ms));
        println!("{}: {}", "p99".bold().blue(), format!("{:.2}ms", result.p99_latency_ms));
        println!("{}: {}", "connections".bold().blue(), result.connections);
        println!("{}: {}", "count".bold().blue(), result.count);
    }
}

fn enable_ansi_support() {
    #[cfg(windows)]
    {
        let _ = colored::control::set_virtual_terminal(true);
    }
}
