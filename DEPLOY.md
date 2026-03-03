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
2. Build                 →  cargo build --release
3. Reload service        →  launchctl unload/load LaunchAgent
4. Migrations            →  SQLx auto-applies on startup
5. Health check          →  /health returns 200
```

If the build fails, the engine is **not** reloaded — the previous binary
continues running.

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

The binary is rebuilt in place. To rollback:

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

## Failure Modes

| Failure | What happens | Action |
|---------|-------------|--------|
| Backup fails pre-deploy | **Build aborted** — old binary keeps running | Fix backup script, check Docker container status |
| Build fails | Old binary keeps running | Check `/tmp/covalence-rebuild.log` |
| Migration fails on startup | Engine panics, launchd restarts (10s throttle) | Check stderr log, fix migration, restart |
| Health check fails | Heartbeat alerts to Discord | Check logs, restart manually if needed |

## Changelog

- **2026-03-03**: Created. Added pre-deploy backup gate (Chris requirement: backup before every deployment). Documented full deployment flow.
