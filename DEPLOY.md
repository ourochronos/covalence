# Covalence Deployment

## Overview

Covalence deploys automatically on commit via a post-commit git hook. The hook
detects changes to `engine/`, backs up the database, rebuilds the release binary,
and reloads the LaunchAgent. Migrations are applied automatically on startup by
SQLx.

## Architecture

| Component | Detail |
|-----------|--------|
| **Binary** | `engine/target/release/covalence-engine` (Rust) |
| **Service** | `ai.ourochronos.covalence-engine` LaunchAgent (KeepAlive) |
| **Database** | PostgreSQL in Docker (`covalence-pg`, port 5434) |
| **Migrations** | SQLx — auto-applied on engine startup |
| **Backups** | `~/.openclaw/backups/covalence/` — scheduled 3×/day + pre-deploy |
| **Health** | `GET http://127.0.0.1:8430/health` → 200 |

## Deployment Flow

Every commit to `main` that touches `engine/` triggers this sequence
automatically via `.git/hooks/post-commit`:

```
1. Pre-deploy backup     →  pg_dump covalence-pg (full SQL dump, gzipped)
2. Save prev binary      →  copy current binary → covalence-engine-prev
3. Build                 →  cargo build --release
4. Reload service        →  launchctl unload/load LaunchAgent
5. Migrations            →  SQLx auto-applies on startup
6. Health-check polling  →  poll /health up to 10×, 1s apart
7. Rollback (if needed)  →  restore covalence-engine-prev + reload on failure
```

If the **build fails**, the engine is **not** reloaded — the previous binary
continues running.

If the **health checks all fail** after reload, the hook automatically restores
`covalence-engine-prev` and reloads the LaunchAgent (rollback). The outcome is
logged to `/tmp/covalence-rebuild.log`.

### Step Details

#### 1. Pre-deploy Backup

The existing backup script runs before any build:

```bash
~/.openclaw/backups/covalence/backup.sh
```

This produces a timestamped, gzipped SQL dump in
`~/.openclaw/backups/covalence/`. Retention: 7 days. If the backup fails,
the deploy is **aborted** — the build does not proceed.

#### 2. Build

```bash
cargo build --release --manifest-path engine/Cargo.toml
```

#### 3. Reload

```bash
launchctl unload ~/Library/LaunchAgents/ai.ourochronos.covalence-engine.plist
sleep 1
launchctl load ~/Library/LaunchAgents/ai.ourochronos.covalence-engine.plist
```

#### 4. Migrations

SQLx runs any pending migrations from `engine/migrations/` on startup.
No manual intervention needed. If a migration fails, the engine will panic
and launchd will restart it (ThrottleInterval: 10s). Check stderr log.

#### 5. Health Check

```bash
curl -sf http://127.0.0.1:8430/health
```

The heartbeat (jane-ops, every 10 minutes) confirms `/health` 200 on every
beat and reports failures to Discord #main-status.

## Scheduled Backups

In addition to pre-deploy backups, covalence-pg is dumped on a schedule:

| Time (local) | Trigger |
|--------------|---------|
| 00:05 | LaunchAgent `com.openclaw.backup.covalence` |
| 08:05 | LaunchAgent `com.openclaw.backup.covalence` |
| 16:05 | LaunchAgent `com.openclaw.backup.covalence` |

Backup location: `~/.openclaw/backups/covalence/covalence_backup_YYYYMMDD_HHMMSS.sql.gz`
Retention: 7 days (auto-pruned).

## Manual Deployment

If the post-commit hook is bypassed or you need to deploy manually:

```bash
# 1. Backup first — always
~/.openclaw/backups/covalence/backup.sh

# 2. Build
cd ~/projects/covalence
cargo build --release --manifest-path engine/Cargo.toml

# 3. Reload
launchctl kickstart -k gui/$(id -u)/ai.ourochronos.covalence-engine

# 4. Verify
sleep 3
curl -sf http://127.0.0.1:8430/health && echo " ✓ healthy" || echo " ✗ UNHEALTHY"
```

## Rollback

### Automatic rollback (post-commit hook)

The post-commit hook saves the current binary to `covalence-engine-prev` before
every build. If the new binary fails all 10 health-check polls, the hook:

1. Copies `covalence-engine-prev` back over `covalence-engine`.
2. Reloads the LaunchAgent.
3. Logs the result to `/tmp/covalence-rebuild.log`.

### Manual rollback

To rollback to the saved previous binary without reverting the commit:

```bash
cp engine/target/release/covalence-engine-prev \
   engine/target/release/covalence-engine
launchctl kickstart -k gui/$(id -u)/ai.ourochronos.covalence-engine
sleep 3
curl -sf http://127.0.0.1:8430/health && echo " ✓ healthy" || echo " ✗ UNHEALTHY"
```

To rollback via git and rebuild:

1. `git revert <commit>` or `git checkout <known-good-sha>`
2. Rebuild: `cargo build --release --manifest-path engine/Cargo.toml`
3. Reload: `launchctl kickstart -k gui/$(id -u)/ai.ourochronos.covalence-engine`

To restore the database from a backup:

```bash
gunzip -k ~/.openclaw/backups/covalence/covalence_backup_YYYYMMDD_HHMMSS.sql.gz
docker exec -i covalence-pg psql -U covalence -d covalence < covalence_backup_YYYYMMDD_HHMMSS.sql
```

## Logs

| Log | Path |
|-----|------|
| Engine stdout | `~/projects/covalence/engine/logs/engine-stdout.log` |
| Engine stderr | `~/projects/covalence/engine/logs/engine-stderr.log` |
| Rebuild hook | `/tmp/covalence-rebuild.log` |
| Backup | `~/.openclaw/backups/covalence/backup.log` |

## Local Development — Starting the Database

Use `scripts/dev-up.sh` to start just the PostgreSQL container and wait for it
to be ready:

```bash
scripts/dev-up.sh
```

The script:
1. Checks the Docker daemon is running.
2. Runs `docker compose up -d postgres` (idempotent).
3. Polls the container healthcheck until it reports `healthy`.
4. Prints the connection string.

The Covalence engine then runs natively via the LaunchAgent (auto-managed by
the post-commit hook).

To start the full containerised stack (engine in Docker instead of native):

```bash
docker compose --profile full up -d
```

## Failure Modes

| Failure | What happens | Action |
|---------|-------------|--------|
| Backup fails pre-deploy | **Build aborted** — old binary keeps running | Fix backup script, check Docker container status |
| Build fails | Old binary keeps running | Check `/tmp/covalence-rebuild.log` |
| Health checks fail post-deploy | **Automatic rollback** — prev binary restored | Check `/tmp/covalence-rebuild.log` |
| Rollback binary missing | No rollback possible — manual fix required | See Manual Rollback above |
| Migration fails on startup | Engine panics, launchd restarts (10s throttle) | Check stderr log, fix migration, restart |
| Health check fails (runtime) | Heartbeat alerts to Discord | Check logs, restart manually if needed |

## Changelog

- **2026-03-03**: Created. Added pre-deploy backup gate (Chris requirement: backup before every deployment). Documented full deployment flow.
- **2026-03-03**: feat(#95) IaC Phase 1 — rollback in post-commit hook (10× health poll, auto-restore prev binary); docker-compose.yml profiles (postgres default, engine opt-in with `--profile full`); `scripts/dev-up.sh` convenience script.
