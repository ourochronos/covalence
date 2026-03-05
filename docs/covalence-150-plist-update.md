# Covalence #150: LaunchAgent plist update

## Change Required

Update `~/Library/LaunchAgents/ai.ourochronos.covalence-engine.plist` to route LLM calls through the Covalence plugin inference proxy.

## Current Configuration

```xml
<key>OPENAI_BASE_URL</key>
<string>https://api.openai.com/v1</string>
```

## New Configuration

```xml
<key>OPENAI_BASE_URL</key>
<string>http://localhost:18789/covalence/v1</string>
```

## Why This Change

The Covalence engine currently hits OpenAI directly for embeddings and chat completions, bypassing subscription-based providers. The plugin already has a working OpenAI-compatible proxy at `localhost:18789/covalence/v1`. By updating `OPENAI_BASE_URL`, all LLM calls will be routed through the proxy, enabling:

1. Use of subscription-based models (GitHub Copilot, etc.)
2. Centralized credential management through the OpenClaw gateway
3. Request logging and monitoring via the plugin proxy

## Environment Variables in Plist

The plist already sets:
- `COVALENCE_CHAT_MODEL=gpt-4.1-mini` (now wired up in main.rs)
- `COVALENCE_EMBED_MODEL=text-embedding-3-small` (already wired up)
- `OPENAI_API_KEY` (will be passed to proxy for auth)

## Apply the Change

```bash
# 1. Stop the service
launchctl unload ~/Library/LaunchAgents/ai.ourochronos.covalence-engine.plist

# 2. Edit the plist
# Change OPENAI_BASE_URL to http://localhost:18789/covalence/v1

# 3. Reload the service
launchctl load ~/Library/LaunchAgents/ai.ourochronos.covalence-engine.plist
```

## Verification

After reloading, check logs:
```bash
tail -f ~/projects/covalence/engine/logs/engine-stderr.log
```

Look for successful embedding/chat requests going through the proxy endpoint.

## Notes

- This plist file is outside the repository (in `~/Library/LaunchAgents/`)
- The change is documented here for manual application
- After the plist change, the engine will route through the plugin proxy automatically
