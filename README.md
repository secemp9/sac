# sac

A modified fork of [nac](https://github.com/arcee-ai/nac) — a small coding agent with a terminal UI.

## What's different

This fork adds:

- **Stream panel** — live model response streaming with expandable events
- **Slash commands** with autocompletion popup and a goal system
- **Prompt history** — Up/Down arrow cycling in the composer
- **Session persistence** — timeline events are saved and restored on resume
- **Improved copy/selection** across all panels
- **Enriched tool descriptions** and file ops guidance in the worker system prompt
- **Deep diagnostics logging** for TUI-visible errors
- **ChatGPT Codex auth** backend (device-code flow)
- **Compact TUI mode** (`--compact` / `--full`)
- **TOML-backed runtime config**
- **`sac upgrade`** command
- **Podman sandbox** support
- Various scrolling, rendering, and markdown fixes

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/secemp9/sac/main/scripts/install.sh | sh
```

## Usage

Set `OPENAI_API_KEY`, then run `sac`.

```sh
sac            # full TUI
sac --compact  # single-column layout
sac --full     # override a compact config default
```

For ChatGPT Codex auth instead of an API key:

```sh
sac codex-auth login
sac --backend chatgpt-codex-responses
```

Optional env vars:
- `OPENAI_BASE_URL`
- `OPENAI_MODEL`

## Config

Config lives at `~/.config/sac/config.toml` (or `$SAC_HOME/config.toml`). CLI args and env vars override TOML defaults.

```toml
[ui]
mode = "full"

[model]
backend = "openai-responses"
model = "gpt-5.5"
base_url = "https://api.openai.com/v1"
reasoning_effort = "xhigh"
api_key_env = "OPENAI_API_KEY"

[worker]
thread_timeout_secs = 3600

[mcp_servers.exa_web_search]
enabled = true
transport = "streamable_http"
url = "https://mcp.exa.ai/mcp"
```

Supported MCP transports: `stdio` and `streamable_http`. String values support `${ENV_VAR}` expansion.

## Sessions

`AGENTS.md` is loaded hierarchically from the project and globally from `SAC_HOME` / `~/.config/sac`. Sessions are stored in `.sac/store.db` by default.

```sh
sac resume            # session picker
sac resume --last     # most recent session
sac resume SESSION_ID # specific session
```

## Sandbox

Run tools inside a Podman sandbox:

```sh
sac --sandbox
```

Options:
- `--no-mount-cwd` — skip default CWD mount
- `--mount HOST:GUEST` — read-write mount
- `--mount-ro HOST:GUEST` — read-only mount
- `--sandbox-image IMAGE` — override default image

## Upgrade

```sh
sac upgrade
```

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/secemp9/sac/main/scripts/uninstall.sh | sh
```
