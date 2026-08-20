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

3. **Create the credentials file** (mode 600, so the proxy admin token is not world-readable):

   ```bash
   sudo mkdir -p /etc/tamad
   sudo tee /etc/tamad/tamad.env >/dev/null <<'EOF'
   TAMA_URL=http://192.168.1.10:18910
   TAMA_TOKEN=<proxy admin token>
   EOF
   sudo chmod 600 /etc/tamad/tamad.env
   ```

   Unset CLI flags fall back to tamad defaults (hostname as `--name`,
   `grpc://<name>:50051` as the public URL, `$HOME/.tama` data dir). If the
   proxy cannot resolve this host's hostname, add `--public-url grpc://<ip>:50051`
   to the `ExecStart` line in the unit.

4. **Enable and start**:

   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable --now tamad
   ```

## Verify

```bash
# Info-level startup lines:
journalctl -u tamad -f
# expect: "Starting tamad daemon" and "Tamad token ready"
# (the registration-success line is debug-level and hidden by default —
#  verify registration with the proxy API instead)

# On the proxy machine:
curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/tama/v1/tamads" | jq
# then force a real health check (proxy calls the tamad's HealthCheck RPC):
curl -s -X POST -H "Authorization: Bearer $TAMA_TOKEN" \
  "$TAMA_URL/tama/v1/tamads/<uuid>/health" | jq
# → {"status":"online"}
```

If `/etc/tamad/tamad.env` is missing, tamad still starts but disables
self-registration (journal: `TAMA_URL/TAMA_TOKEN not fully set —
self-registration disabled`) and never appears in the proxy — create the
file and `systemctl restart tamad`.

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
- **Running as non-root (optional):** the unit defaults to root because
  tamad requires `$HOME` and commonly needs docker plus direct GPU device
  access (nvidia render / `kfd` groups). To run as a dedicated user, add
  `User=tamadaemon` / `Group=tamadaemon` under `[Service]`, give that user a
  home dir (token and models land in its `~/.tama`), and add it to the docker
  and GPU device groups — loaded backends inherit that user's device access.
- **Networking:** the tamad must be reachable from the proxy (firewall) on
  the public URL; access is authorized with the per-tamad bearer token.
