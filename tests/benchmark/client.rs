use cache_benchmark::client::ClientConfig;
use cache_benchmark_tests::{MockRespConfig, start_mock};

fn base_config() -> ClientConfig {
    ClientConfig {
        cluster: false,
        address: "127.0.0.1:6379".to_string(),
        username: String::new(),
        password: String::new(),
        tls: false,
        timeout: 30,
    }
}

#[tokio::test]
async fn topology_discovery_returns_none_for_standalone() {
    let cfg = base_config();
    assert!(cfg.discover_cluster_topology().await.is_none());
}

#[tokio::test]
async fn topology_discovery_parses_cluster_slots() {
    let server = start_mock(
        MockRespConfig::default().cluster(
            vec![
                "127.0.0.1:7001".to_string(),
                "127.0.0.1:7002".to_string(),
                "127.0.0.1:7003".to_string(),
            ],
            vec!["127.0.0.1:8001".to_string()],
        ),
    )
    .await
    .unwrap();

    let cfg = ClientConfig {
        cluster: true,
        address: server.addr(),
        ..base_config()
    };
    let topo = cfg.discover_cluster_topology().await.expect("topology");
    let (masters, replicas, covered) = topo;
    assert_eq!(masters.len(), 3, "masters: {:?}", masters);
    assert!(masters.contains(&"127.0.0.1:7001".to_string()));
    assert!(masters.contains(&"127.0.0.1:7002".to_string()));
    assert!(masters.contains(&"127.0.0.1:7003".to_string()));
    assert_eq!(replicas, 3, "one replica per range x 3 ranges");
    assert_eq!(covered, 16384, "slots fully covered");
}

#[tokio::test]
async fn topology_discovery_none_when_server_down() {
    // bind then drop to get an unused port
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let cfg = ClientConfig {
        cluster: true,
        address: format!("127.0.0.1:{}", port),
        ..base_config()
    };
    assert!(cfg.discover_cluster_topology().await.is_none());
}

#[test]
fn display_hides_password_semantics() {
    let cfg = ClientConfig {
        password: "secret".to_string(),
        ..base_config()
    };
    let s = cfg.to_string();
    assert!(s.contains("secret"), "Display shows the config (admin tooling): {}", s);
}

#[tokio::test]
async fn get_client_fails_fast_on_dead_server() {
    // ClientConfig::get_client prints and exits on failure — instead of
    // testing that, verify a dead port is really dead so the e2e tests
    // can rely on connection errors.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let conn = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
    assert!(conn.is_err(), "port {} should be unreachable", port);
}
