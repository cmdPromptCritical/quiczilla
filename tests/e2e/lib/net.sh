#!/usr/bin/env bash
# Network manipulation helpers for Quiczilla E2E tests.
# Requires NET_ADMIN capability inside the container.
set -uo pipefail

# Detect default interface (usually eth0 in Docker/k8s)
default_iface() {
  ip route show default 2>/dev/null | awk '/default/ {print $5}' | head -n 1 || echo "eth0"
}

# Clear any tc qdisc rules on interface
net_clear_impair() {
  local iface="${1:-$(default_iface)}"
  tc qdisc del dev "$iface" root 2>/dev/null || true
}

# Apply network impairment using tc netem:
# Usage: net_impair [--loss <pct>] [--delay <ms>] [--reorder <pct>] [--rate <rate>] [iface]
net_impair() {
  local iface=""
  local loss=""
  local delay=""
  local reorder=""
  local rate=""

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --loss) loss="$2"; shift 2 ;;
      --delay) delay="$2"; shift 2 ;;
      --reorder) reorder="$2"; shift 2 ;;
      --rate) rate="$2"; shift 2 ;;
      *) iface="$1"; shift ;;
    esac
  done

  [[ -z "$iface" ]] && iface="$(default_iface)"
  net_clear_impair "$iface"

  local cmd="tc qdisc add dev $iface root netem"
  [[ -n "$delay" ]] && cmd="$cmd delay ${delay}ms"
  [[ -n "$loss" ]] && cmd="$cmd loss ${loss}%"
  [[ -n "$reorder" ]] && cmd="$cmd reorder ${reorder}% 25%"
  [[ -n "$rate" ]] && cmd="$cmd rate ${rate}"

  log "Applying network impairment: $cmd"
  $cmd
}

# Flush all custom iptables rules applied during tests
net_clear_iptables() {
  iptables -F OUTPUT 2>/dev/null || true
}

# Block all outbound UDP to a specific target IP (forces fallback to SSH or drops QUIC)
net_block_udp_to_ip() {
  local target_ip="$1"
  log "Blocking outbound UDP to $target_ip"
  iptables -A OUTPUT -p udp -d "$target_ip" -j DROP
}

# Block outbound UDP to a specific target IP and port (e.g. STUN 3478)
net_block_udp_port() {
  local target_ip="$1"
  local port="${2:-3478}"
  log "Blocking outbound UDP to $target_ip:$port"
  iptables -A OUTPUT -p udp -d "$target_ip" --dport "$port" -j DROP
}

# Unblock specific rule if needed or reset chain
net_reset_all() {
  net_clear_iptables
  net_clear_impair "$(default_iface)"
}
