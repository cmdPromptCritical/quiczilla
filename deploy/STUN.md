# Operating Quiczilla's Default STUN Service

Quiczilla uses STUN only to discover public UDP candidates. It never sends file
data through this service: after candidate discovery, the QUIC connection is
peer-to-peer and protected by mTLS.

Use coturn in STUN-only mode for a public service. Do not expose the
development-only Python server in `scripts/stun_server.py` to the Internet.

## Linux deployment

1. Provision a small public VM with a stable IPv4 address. Give it a DNS record
   such as `stun.example.com`.
2. Install coturn and copy the included configuration:

   ```sh
   sudo apt update
   sudo apt install coturn
   sudo install -m 0644 deploy/stun/turnserver.conf /etc/turnserver.conf
   sudo install -m 0644 deploy/stun/quiczilla-stun.service /etc/systemd/system/quiczilla-stun.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now quiczilla-stun
   ```

3. Allow only UDP 3478 in the VM firewall and cloud security group. Do not
   expose TCP 3478, TLS ports, or TURN relay ranges.
4. Add an ingress rate limit at the cloud firewall or host firewall. STUN is
   intentionally unauthenticated for interoperability, so rate limiting and
   provider DDoS protection are important.
5. Monitor UDP reachability and service logs. Retain only short-lived,
   privacy-conscious logs: STUN requests reveal source IP addresses and timing.

The supplied configuration uses `stun-only`, disables relay listeners, hides
the software attribute, and disables the management CLI. Coturn should run as
its package-provided unprivileged `turnserver` account.

## Shipping the default endpoint

After DNS and monitoring are ready, set the GitHub repository Actions variable
`QUICZILLA_DEFAULT_STUN_SERVER` to:

```text
stun.example.com:3478
```

The release workflow embeds that value into newly built CLIs. Users can still
override it with `--stun-server` or `QUICZILLA_STUN_SERVER`; forced
`--transport direct`, `manual`, and `ssh` modes bypass the default STUN service.

Run at least two independent STUN endpoints before treating this as a highly
available service. A default-STUN outage should only make Quiczilla select a
direct candidate or SSH fallback; it must not expose transfer contents or block
an intentional SSH-only transfer.
