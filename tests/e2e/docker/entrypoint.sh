#!/usr/bin/env bash
# Container entrypoint for the Quiczilla E2E image.
#
#   server  Start sshd for user `qz`. The authorized key comes from
#           $QZ_AUTHORIZED_KEY or /keys/id_ed25519.pub (shared compose volume).
#           No worker is placed on PATH: bootstrap must upload a managed bundle.
#   client  Put the CLI on PATH, create or reuse /keys/id_ed25519, and write an
#           SSH client config for $QZ_SERVER_HOST. Then run "$@" or idle.
set -euo pipefail

role="${1:-client}"
shift || true

: "${QZ_SERVER_HOST:=server}"
: "${QZ_SERVER_USER:=qz}"
: "${QZ_SERVER_SSH_PORT:=22}"
: "${QZ_SSH_PORT:=22}"
key_dir="${QZ_KEY_DIR:-/keys}"

start_server() {
  ssh-keygen -A >/dev/null
  install -d -m 0700 -o qz -g qz /home/qz/.ssh

  local pub=""
  for _ in $(seq 1 120); do
    if [[ -n "${QZ_AUTHORIZED_KEY:-}" ]]; then
      pub="$QZ_AUTHORIZED_KEY"
      break
    fi
    if [[ -s "$key_dir/id_ed25519.pub" ]]; then
      pub="$(cat "$key_dir/id_ed25519.pub")"
      break
    fi
    sleep 0.5
  done
  if [[ -z "$pub" ]]; then
    echo "server: no client public key (set QZ_AUTHORIZED_KEY or share $key_dir)" >&2
    exit 1
  fi
  printf '%s\n' "$pub" > /home/qz/.ssh/authorized_keys
  chown qz:qz /home/qz/.ssh/authorized_keys
  chmod 0600 /home/qz/.ssh/authorized_keys

  cat > /etc/ssh/sshd_config.d/quiczilla-e2e.conf <<EOF
Port ${QZ_SSH_PORT}
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
AllowUsers qz
MaxStartups 64:30:128
MaxSessions 64
EOF
  # Readiness marker for the compose health check.
  touch /run/qz-e2e-server-ready
  exec /usr/sbin/sshd -D -e
}

start_client() {
  ln -sf /opt/quiczilla/quiczilla /usr/local/bin/quic
  ln -sf /opt/quiczilla/quiczilla /usr/local/bin/quiczilla

  if [[ ! -s "$key_dir/id_ed25519" ]]; then
    if [[ -w "$key_dir" ]]; then
      (umask 077 && ssh-keygen -q -t ed25519 -N '' -C qz-e2e -f "$key_dir/id_ed25519")
    else
      echo "client: $key_dir/id_ed25519 is missing and $key_dir is read-only" >&2
      exit 1
    fi
  fi
  # Secrets may be mounted read-only with broad modes; ssh requires 0600.
  install -d -m 0700 /root/.ssh
  install -m 0600 "$key_dir/id_ed25519" /root/.ssh/qz_e2e_key

  cat > /root/.ssh/config <<EOF
Host ${QZ_SERVER_HOST}
  User ${QZ_SERVER_USER}
  Port ${QZ_SERVER_SSH_PORT}
  IdentityFile /root/.ssh/qz_e2e_key
  IdentitiesOnly yes
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR
  ConnectTimeout 15
EOF
  chmod 0600 /root/.ssh/config

  if [[ $# -gt 0 ]]; then
    exec "$@"
  fi
  exec sleep infinity
}

case "$role" in
  server) start_server ;;
  client) start_client "$@" ;;
  *) exec "$role" "$@" ;;
esac
