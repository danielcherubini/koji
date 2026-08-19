# tamad systemd unit

A ready-to-install system unit for running the **tamad** daemon on an
inference host. The proxy never installs or restarts tamads (ADR-0010) —
this is the manual, per-host deployment path.

## Install

1. **Install the binary** (build from the same tag/commit as the proxy):

   ```bash
   cargo build --release -p tamad
   sudo install -m 755 target/release/tamad /usr/local/bin/tamad
   ```

2. **Install the unit**:

   ```bash
   sudo install -m 644 tamad.service /etc/systemd/system/tamad.service
   ```

3. **Replace the two placeholders** in `/etc/systemd/system/tamad.service`:

   - `TAMA_URL` — the proxy base URL, e.g. `http://192.168.1.10:18910`
   - `TAMA_TOKEN` — the proxy admin token

   Unset CLI flags fall back to tamad defaults (hostname as `--name`,
   `grpc://<name>:50051` as the public URL, `$HOME/.tama` data dir). If the
   proxy cannot resolve this host's hostname, add `--public-url grpc://<ip>:50051`
   to the `ExecStart` line.

4. **Enable and start**:

   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable --now tamad
   ```

## Verify

```bash
# Daemon announces itself at startup:
journalctl -u tamad -f
# expect: "Starting tamad daemon" → "Tamad token ready" →
#         "Self-registered with proxy (created)"

# On the proxy machine:
curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/tama/v1/tamads" | jq
# then force a real health check (proxy calls the tamad's HealthCheck RPC):
curl -s -X POST -H "Authorization: Bearer $TAMA_TOKEN" \
  "$TAMA_URL/tama/v1/tamads/<uuid>/health" | jq
# → {"status":"online"}
```

If the placeholder token was left in place, the journal shows
`Self-registration attempt failed; will retry` every 5 minutes and the
tamad never appears in the proxy — fix the token and
`systemctl restart tamad`.

## Notes

- **Token stability:** the tamad generates its own bearer token once and
  persists it at `$HOME/.tama/tamad.token` (mode 0600). It survives restarts
  and binary replacements, and the proxy's stored copy refreshes on every
  registration (idempotent upsert by name, every 5 minutes).
- **Upgrading:** replace `/usr/local/bin/tamad`, then `systemctl restart tamad`.
  On SIGTERM the daemon kills every backend process group before exiting, so
  no orphaned engine processes survive the restart.
- **Version skew:** keep the tamad at the same version as the proxy — they
  speak a shared gRPC/HTTP protocol.
- **Networking:** the tamad must be reachable from the proxy (firewall) on
  the public URL; access is authorized with the per-tamad bearer token.
