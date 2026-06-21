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
- **`nac upgrade`** command
- **Podman sandbox** support
- Various scrolling, rendering, and markdown fixes

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/secemp9/sac/main/scripts/install.sh | sh
```

## Usage

Set `OPENAI_API_KEY`, then run `nac`.

```sh
nac            # full TUI
nac --compact  # single-column layout
nac --full     # override a compact config default
```

For ChatGPT Codex auth instead of an API key:

```sh
nac codex-auth login
nac --backend chatgpt-codex-responses
```

Optional env vars:
- `OPENAI_BASE_URL`
- `OPENAI_MODEL`

## Config

Config lives at `~/.config/nac/config.toml` (or `$NAC_HOME/config.toml`). CLI args and env vars override TOML defaults.

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

`AGENTS.md` is loaded hierarchically from the project and globally from `NAC_HOME` / `~/.config/nac`. Sessions are stored in `.nac/store.db` by default.

```sh
nac resume            # session picker
nac resume --last     # most recent session
nac resume SESSION_ID # specific session
```

## Sandbox

Run tools inside a Podman sandbox:

```sh
nac --sandbox
```

Options:
- `--no-mount-cwd` — skip default CWD mount
- `--mount HOST:GUEST` — read-write mount
- `--mount-ro HOST:GUEST` — read-only mount
- `--sandbox-image IMAGE` — override default image

## Upgrade

```sh
nac upgrade
```

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/secemp9/sac/main/scripts/uninstall.sh | sh
```
