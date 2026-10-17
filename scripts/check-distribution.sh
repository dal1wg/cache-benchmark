#!/bin/bash
# 检查集群各 master 的 total_commands_processed，验证负载分布是否均匀。
# 用法：HOST=192.168.102.34 PORT=16385 ./scripts/check-distribution.sh <ip1> <ip2> ...
set -e
PORT="${PORT:-16385}"
HOSTS=("$@")
if [ ${#HOSTS[@]} -eq 0 ]; then
    HOSTS=(192.168.102.34 192.168.102.35 192.168.102.37)
fi

for host in "${HOSTS[@]}"; do
    if command -v redis-cli >/dev/null 2>&1; then
        v=$(redis-cli -h "$host" -p "$PORT" INFO stats 2>/dev/null | grep -o 'total_commands_processed:[0-9]*' | head -1 | cut -d: -f2)
    else
        v=$( (exec 3<>/dev/tcp/$host/$PORT && printf 'INFO stats\r\n' >&3 && timeout 3 head -c 4000 <&3) 2>/dev/null | grep -o 'total_commands_processed:[0-9]*' | head -1 | cut -d: -f2)
    fi
    echo "$host:$PORT total_commands_processed=$v"
done
