#!/usr/bin/env bash
# Gather benchmark-relevant system information.
set -euo pipefail

sep() { echo; echo "=== $* ==="; }

sep "CPU"
lscpu | grep -E "Model name|Socket|Core|Thread|NUMA|MHz|cache"

sep "Memory"
grep -E "MemTotal|MemFree|SwapTotal" /proc/meminfo
dmidecode -t memory 2>/dev/null | grep -E "Size|Speed|Type:|Rank" | grep -v "No Module" || echo "(dmidecode requires root for full memory detail)"

sep "Storage"
lsblk -o NAME,SIZE,ROTA,TYPE,MODEL,TRAN | grep -v "^loop"
cat /sys/block/*/queue/scheduler 2>/dev/null && true

sep "Filesystem (benchmark paths)"
df -Th .
mount | grep "$(df . | tail -1 | awk '{print $1}')" || true
findmnt -n -o OPTIONS "$(df . | tail -1 | awk '{print $1}')" 2>/dev/null || true

sep "OS and kernel"
uname -r
cat /etc/os-release | grep -E "^(NAME|VERSION|ID)="

sep "CPU frequency governor"
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo "N/A"
cat /sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference 2>/dev/null || echo "N/A"
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null || echo "N/A"
cat /sys/devices/system/cpu/cpufreq/policy0/scaling_min_freq 2>/dev/null || echo "N/A"
cat /sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq 2>/dev/null || echo "N/A"

sep "SMT / Hyper-Threading"
cat /sys/devices/system/cpu/smt/active 2>/dev/null || echo "N/A"
lscpu | grep "Thread(s) per core"

sep "ASLR"
cat /proc/sys/kernel/randomize_va_space

sep "CPU affinity (this process)"
taskset -cp $$ 2>/dev/null || echo "N/A"

sep "NUMA topology"
numactl --hardware 2>/dev/null || lscpu | grep "NUMA" || echo "N/A"

sep "Transparent huge pages"
cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || echo "N/A"
