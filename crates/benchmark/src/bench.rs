use crate::{BenchResult, Case};
use awaitgroup::WaitGroup;
use colored::Colorize;
use std::io::Write;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::{select, task};

use crate::auto_connection::{AutoConnection, ConnLimiter};
use crate::client::ClientConfig;
use crate::shared_context::SharedContext;
use cb_histogram::{Histogram, HistogramData};
use governor::{DefaultDirectRateLimiter, Jitter, Quota};

async fn run_commands_on_single_thread(conn_limiter: Arc<ConnLimiter>, qps_limiter: Arc<Option<DefaultDirectRateLimiter>>, config: ClientConfig, case: Case, context: SharedContext, layout: Option<Arc<crate::client::ClusterLayout>>) {
    let local = task::LocalSet::new();
    for _ in 0..conn_limiter.total_conn {
        let conn_limiter = conn_limiter.clone();
        let qps_limiter = qps_limiter.clone();
        let config = config.clone();
        let case = case.clone();
        let mut context = context.clone();
        let layout = layout.clone();
        local.spawn_local(async move {
            let mut cmd = case.command.clone_box();
            let conn_limiter = conn_limiter.clone();
            select! {
                _ = conn_limiter.wait_new_conn() =>{}
                _ = context.wait_stop() => {
                    return;
                }
            }

            if case.short_connection {
                // Short connection mode: create a new connection for each command
                // Note: pipeline must be 1 and load must be false in short_connection mode
                assert_eq!(case.pipeline, 1, "pipeline must be 1 in short_connection mode");
                loop {
                    let pipeline_cnt = context.fetch(case.pipeline);
                    if pipeline_cnt == 0 {
                        context.stop();
                        break;
                    }

                    // Rate limiting check
                    match qps_limiter.as_ref() {
                        Some(limiter) => {
                            let j = Jitter::up_to(std::time::Duration::from_micros(1_000_000_u64 * conn_limiter.total_conn / case.target.max(1)));
                            let mut remaining = pipeline_cnt;
                            while remaining > 0 {
                                let tokens = remaining.min(case.target).min(u32::MAX as u64) as u32;
                                limiter.until_n_ready_with_jitter(NonZeroU32::new(tokens).unwrap(), j).await.expect("tokens fit quota");
                                remaining -= tokens as u64;
                            }
                        }
                        None => {}
                    }

                    // Create new connection and execute single command
                    let mut client = config.get_client_with_layout(layout.clone()).await;
                    let instant = std::time::Instant::now();
                    let mut p = Vec::new();
                    p.push(cmd.gen_cmd());
                    let should_count = client.run_commands(p).await;
                    let duration = instant.elapsed().as_micros() as u64;
                    // Explicitly drop connection to close it immediately
                    drop(client);

                    if should_count {
                        context.histogram.record(duration);
                    }
                }
            } else {
                // Long connection mode: reuse connections
                let mut client = config.get_client_with_layout(layout.clone()).await;
                loop {
                    let pipeline_cnt = context.fetch(case.pipeline);
                    if pipeline_cnt == 0 {
                        context.stop();
                        break;
                    }

                    // Rate limiting check first
                    match qps_limiter.as_ref() {
                        Some(limiter) => {
                            let j = Jitter::up_to(std::time::Duration::from_micros(1_000_000_u64 * conn_limiter.total_conn / case.target.max(1)));
                            let mut remaining = pipeline_cnt;
                            while remaining > 0 {
                                let tokens = remaining.min(case.target).min(u32::MAX as u64) as u32;
                                limiter.until_n_ready_with_jitter(NonZeroU32::new(tokens).unwrap(), j).await.expect("tokens fit quota");
                                remaining -= tokens as u64;
                            }
                        }
                        None => {}
                    }

                    // prepare pipeline
                    let mut p = Vec::new();
                    for _ in 0..pipeline_cnt {
                        if context.is_loading {
                            p.push(cmd.gen_cmd_with_lock());
                        } else {
                            p.push(cmd.gen_cmd());
                        }
                    }
                    let instant = std::time::Instant::now();
                    let should_count = client.run_commands(p).await;
                    let duration = instant.elapsed().as_micros() as u64;
                    if should_count {
                        context.histogram.record_n(duration, pipeline_cnt);
                    }
                }
            }
        });
    }
    local.await;
}

fn snapshot(histograms: &[Arc<Histogram>]) -> HistogramData {
    let mut merged = HistogramData::empty();
    for histogram in histograms {
        merged.merge(&HistogramData::from_histogram(histogram));
    }
    merged
}

fn wait_finish( mut auto_connection: AutoConnection, mut context: SharedContext, mut wg: WaitGroup, quiet: bool, histograms: Vec<Arc<Histogram>>) -> BenchResult {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let mut overall_time = std::time::Instant::now();
        let mut baseline = HistogramData::empty();
        let mut log_instance = std::time::Instant::now();
        let mut log_last_cnt = 0;
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(233));
        if auto_connection.ready {
            context.start_timer();
        }
        loop {
            select! {
                _ = interval.tick() => {}
                _ = wg.wait() => break,
            }
            let data = snapshot(&histograms);
            let count = data.cnt();
            if !quiet {
                let qps = count.saturating_sub(log_last_cnt) as f64 / log_instance.elapsed().as_secs_f64();
                println!("qps: {:.0}, conn: {}, count: {}{}", qps, auto_connection.active_conn(), count,
                    if auto_connection.ready { "" } else { " (auto connection warmup)" });
                std::io::stdout().flush().unwrap();
            }
            log_last_cnt = count;
            log_instance = std::time::Instant::now();
            if !auto_connection.ready {
                auto_connection.adjust_count(count);
                if auto_connection.ready {
                    // Exclude the same warmup samples from count, latency and
                    // QPS, rather than mixing different measurement windows.
                    baseline = snapshot(&histograms);
                    overall_time = std::time::Instant::now();
                    context.start_timer();
                }
            }
        }
        let mut histogram = snapshot(&histograms);
        histogram.subtract(&baseline);
        let elapsed_secs = overall_time.elapsed().as_secs_f64();
        BenchResult {
            qps: histogram.cnt() as f64 / elapsed_secs,
            avg_latency_ms: histogram.avg_us() as f64 / 1_000.0,
            p99_latency_ms: histogram.percentile_us(0.99) as f64 / 1_000.0,
            connections: auto_connection.active_conn(),
            count: histogram.cnt(),
            elapsed_secs,
            histogram,
            cluster_nodes: context.take_cluster_nodes(),
        }
    })
}

pub fn do_benchmark(client_config: ClientConfig, cores: Vec<u16>, case: Case, load: bool, quiet: bool) -> BenchResult {
    // cluster mode: discover topology up-front via CLUSTER SLOTS so the user can
    // confirm the full topology was recognized before traffic starts
    let mut cluster_nodes: Vec<String> = Vec::new();
    let mut cluster_layout = None;
    if client_config.cluster {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let topo = rt.block_on(client_config.discover_cluster_layout());
        match topo {
            Some(layout) => {
                let masters = &layout.masters;
                let replicas = layout.replicas;
                let covered = layout.covered;
                if !quiet {
                    println!("{}: {} masters, {} replicas, slots {}/{} covered", "cluster topology".bold().blue(), masters.len(), replicas, covered, 16384);
                    for m in masters {
                        println!("  {} {}", "master".bold().blue(), m);
                    }
                }
                cluster_nodes = masters.clone();
                cluster_layout = Some(Arc::new(layout));
            }
            None => {
                eprintln!("Warning: failed to discover cluster topology from seed {}, continuing anyway (the cluster client will retry)", client_config.address);
            }
        }
    }

    if !quiet {
        println!("{}: {}", "command".bold().blue(), case.command.to_string().green().bold());
        println!("{}: {}", "connections".bold().blue(), if case.connections == 0 { "auto".to_string() } else { case.connections.to_string() });
        println!("{}: {}", "count".bold().blue(), case.count);
        println!("{}: {}", "target".bold().blue(), if case.target == 0 { "unlimited".to_string() } else { case.target.to_string() });
        println!("{}: {}", "seconds".bold().blue(), case.seconds);
        println!("{}: {}", "pipeline".bold().blue(), case.pipeline);
        println!("{}: {}", "short_connection".bold().blue(), case.short_connection);
        if client_config.cluster {
            println!("{}: {} (requests are routed to master nodes by key hash slot)", "cluster".bold().blue(), "enabled".to_string());
        }
    }

    // calc connections
    let auto_connection = AutoConnection::with_limit(case.connections, cores.len() as u64, case.max_connections);

    // calc target qps
    let qps_limiter = Arc::new(if case.target > 0 {
        let limiter = DefaultDirectRateLimiter::direct(Quota::per_second(NonZeroU32::new(case.target as u32).unwrap()));
        while limiter.check().is_ok() {}
        Some(limiter)
    } else {
        None
    });

    let mut thread_handlers = Vec::new();
    let mut histograms = Vec::new();
    let wg = WaitGroup::new();
    let core_ids = core_affinity::get_core_ids().unwrap();
    let context = SharedContext::new(case.count, case.seconds, load);
    if !cluster_nodes.is_empty() {
        context.set_cluster_nodes(cluster_nodes);
    }
    for inx in 0..cores.len() {
        let client_config = client_config.clone();
        let case = case.clone();
        let mut context = context.clone();
        context.histogram = Arc::new(Histogram::new());
        histograms.push(context.histogram.clone());
        let layout = cluster_layout.clone();
        let wk = wg.worker();
        if cores[inx] as usize >= core_ids.len() {
            eprintln!("Error: core {} is out of range, only {} cores available", cores[inx], core_ids.len());
            std::process::exit(1);
        }
        let core_id = core_ids[cores[inx] as usize];
        let conn_limiter = auto_connection.limiters[inx].clone();
        let qps_limiter = qps_limiter.clone();
        let thread_handler = std::thread::spawn(move || {
            core_affinity::set_for_current(core_id); // not work on Apple Silicon
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                run_commands_on_single_thread(conn_limiter, qps_limiter, client_config, case, context, layout).await;
                wk.done();
            });
        });
        thread_handlers.push(thread_handler);
    }

    // log thread
    let result = wait_finish(auto_connection, context, wg, quiet, histograms);

    // join all threads
    for thread_handler in thread_handlers {
        loop {
            if thread_handler.is_finished() {
                thread_handler.join().unwrap();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    result
}
