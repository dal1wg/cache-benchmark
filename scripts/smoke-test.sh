#!/bin/bash
# 冒烟测试（需可访问的测试环境，默认压测程序在本机）。
# 用法：BIN=~/cb-target/release/cache-benchmark ./scripts/smoke-test.sh
set -e
BIN="${BIN:-cache-benchmark}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-6379}"
CLUSTER_HOST="${CLUSTER_HOST:-$HOST}"
CLUSTER_PORT="${CLUSTER_PORT:-16385}"

echo "=== [standalone] load + GET ==="
$BIN -h "$HOST" -p "$PORT" --load -c 8 -P 16 -n 10000 "SET {key sequence 10000} {value 64}"
$BIN -h "$HOST" -p "$PORT" -s 5 -c 8 -P 16 "GET {key uniform 10000}"

echo "=== [standalone] lua ==="
$BIN -h "$HOST" -p "$PORT" -s 3 -c 4 --lua "local k = bench.key(1000, 'uniform', 'x'); local v = bench.value(32); function generate() return {'SET', k(), v()} end"

echo "=== [cluster] topology + load + GET ==="
$BIN -h "$CLUSTER_HOST" -p "$CLUSTER_PORT" --cluster -s 4 -c 4 "PING"
$BIN -h "$CLUSTER_HOST" -p "$CLUSTER_PORT" --cluster --load -c 8 -P 16 -n 10000 "SET {key sequence 10000} {value 64}"
$BIN -h "$CLUSTER_HOST" -p "$CLUSTER_PORT" --cluster -s 5 -c 8 -P 16 "GET {key uniform 10000}"

echo "=== [cluster] json summary ==="
$BIN -h "$CLUSTER_HOST" -p "$CLUSTER_PORT" --cluster -s 5 -c 8 -P 16 -q --json "GET {key uniform 10000}" > /tmp/cb-result.json
python3 - <<'EOF'
import json
d = json.load(open('/tmp/cb-result.json'))
print('qps: %.1f' % d['qps'])
print('count:', d['count'])
print('avg_ms: %.2f' % d['avg_latency_ms'])
print('p99_ms: %.2f' % d['p99_latency_ms'])
print('cluster_nodes:', d['cluster_nodes'])
EOF

echo "=== ALL SMOKE TESTS PASSED ==="
