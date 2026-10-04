#!/usr/bin/env bash
# X1 NAT Characterisation.
# Queries the STUN server from two separate local ports to observe NAT mapping behavior:
# - If mapping assigns the same external IP & port pattern, NAT is Endpoint Independent (Full/Restricted Cone).
# - If mapping changes per target or allocates unpredictable ports, NAT is Symmetric.
# Note: Redacts discovered addresses; records only classification.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

[[ -n "${QZ_E2E_STUN:-}" ]] || skip "QZ_E2E_STUN not configured"
stun="$(resolve_stun)" || fail "cannot resolve STUN server '${QZ_E2E_STUN}' to IPv4"
prepare_dirs
trap cleanup_remote_dir EXIT

# Test client NAT mapping across two distinct sockets
discover_port() {
  local bind_p="$1"
  python3 - "$stun" "$bind_p" <<'PY'
import socket, sys, struct

stun_host, stun_port = sys.argv[1].split(':')
stun_port = int(stun_port)
local_port = int(sys.argv[2])

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(('0.0.0.0', local_port))
s.settimeout(5.0)

# Binding request: Type=0x0001, Length=0, Magic Cookie=0x2112A442, Transaction ID=12 bytes
tx_id = b'\x12' * 12
req = struct.pack('!HHI12s', 0x0001, 0, 0x2112A442, tx_id)
s.sendto(req, (stun_host, stun_port))

try:
    resp, _ = s.recvfrom(2048)
    msg_type, length, cookie = struct.unpack('!HHI', resp[:8])
    pos = 20
    while pos < len(resp):
        attr_type, attr_len = struct.unpack('!HH', resp[pos:pos+4])
        pos += 4
        if attr_type == 0x0020: # XOR-MAPPED-ADDRESS
            family, xport = struct.unpack('!xBH', resp[pos:pos+4])
            port = xport ^ 0x2112
            print(f"PORT:{port}")
            break
        pos += attr_len
except Exception as e:
    sys.exit(1)
PY
}

p1="$(discover_port 45100 | grep 'PORT:' | cut -d: -f2 || true)"
p2="$(discover_port 45102 | grep 'PORT:' | cut -d: -f2 || true)"

if [[ -n "$p1" && -n "$p2" ]]; then
  diff=$((p2 - p1))
  log "Discovered client port mapping delta: $diff"
  if [[ "$diff" -eq 2 ]]; then
    metric "client_nat_behavior" "sequential_cone"
  else
    metric "client_nat_behavior" "random_or_symmetric"
  fi
else
  log "Could not query client port mappings"
  metric "client_nat_behavior" "unknown"
fi

log "X1 NAT characterisation completed"
