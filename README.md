# SAC

A fork of [nac](https://github.com/arcee-ai/nac), a terminal-based coding agent with an orchestrator/worker architecture. SAC shares NAC's baseline -- threading, worksets, TUI, tools, sessions, sandbox, MCP, skills, AGENTS.md -- but diverges in several areas.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/secemp9/sac/main/scripts/install.sh | sh
```

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/secemp9/sac/main/scripts/uninstall.sh | sh
```

## Upgrade

```sh
sac upgrade
```

## What SAC adds

Features present in SAC but not in NAC:

- **Goal system.** Autonomous multi-turn goal pursuit with token budgets, evidence-based completion audits, blocked audits (3 consecutive turns), a 6-status lifecycle (Active, Paused, Complete, Blocked, UsageLimited, BudgetLimited), and mid-turn steering for budget warnings and objective changes. Managed via `/goal set|show|clear|pause|resume|edit`.
- **Stream panel.** Live model output streaming in the TUI (Ctrl-S).
- **Prompt history.** Up/Down arrow cycling through previous prompts.
- **Slash commands with autocomplete popup.** Commands surface in a popup as you type.
- **Custom commands from .md files.** Place `.md` files with YAML frontmatter in `.sac/commands/` (project) or `~/.config/sac/commands/` (global). Template variables: `$ARGUMENTS`, `$WORKING_DIRECTORY`.
- **Persistent terminal tool.** Named PTY sessions that persist beyond a single `exec_command` call.
- **`sac config` CLI suite.** Subcommands: `show`, `init`, `path`, `log-path`, `logs`, `tail-log`, `doctor`, `reload`.
- **Structured file logging with rotation.** Logs to `$SAC_HOME/logs/`.
- **Additional auth methods.** `--with-api-key`, `--with-access-token`, `--headless` flags for the device-code flow; direct `api_key` field in config.toml.
- **Reasoning controls.** `--reasoning-summary` (auto, concise, detailed) and `--reasoning-context` (current_turn, all_turns) CLI flags.
- **File changes panel.** TUI panel showing modified files (Ctrl-F).
- **Session picker UI.** Interactive session selection on `sac resume`.
- **Conway's Game of Life background animation.**

## What SAC removes

Features present in NAC but not in SAC:

- **No Anthropic/Claude backend.** NAC supports Anthropic models. SAC is OpenAI-compatible APIs only.
- **No web dashboard or HTTP server.** NAC has `nac-web` with a REST API and SSE streaming for managing multiple sessions from a browser. SAC is TUI only.
- **No SSH remote workers.** NAC can dispatch workers to remote machines over SSH. SAC runs workers as local processes only.
- **No workspace diff view.** NAC has full git diff rendering in the TUI. SAC does not.
- **Single crate.** NAC uses a three-crate split. SAC compiles as a single crate and single binary.

## What SAC changes

Where both have the feature but SAC does it differently:

- **Config paths.** `.sac/` and `~/.config/sac/` instead of `.nac/` and `~/.config/nac/`.
- **Store path.** `.sac/store.db` instead of `.nac/store.db`.
- **Binary name.** `sac` instead of `nac`.
- **Env prefix.** `SAC_HOME` instead of `NAC_HOME`.

## Basic usage

```sh
sac                          # start TUI
sac --compact                # compact layout
sac --full                   # full layout
sac -C /path                 # set working directory
sac --sandbox                # run in Podman sandbox
sac --backend <name>         # model backend (openai-responses, chatgpt-codex-responses, deepseek-chat, fireworks-chat)
sac --effort <level>         # reasoning effort
sac resume                   # session picker
sac resume --last            # resume most recent session
```

## Config

Config lives at `~/.config/sac/config.toml` (or `$SAC_HOME/config.toml`). CLI args and env vars override config values.

SAC-specific options not found in NAC:

```toml
[model]
api_key = "sk-..."                    # direct key (alternative to env var)
reasoning_effort = "high"

# Reasoning controls (also settable via CLI flags):
# --reasoning-summary auto|concise|detailed
# --reasoning-context current_turn|all_turns
```

For the full set of shared config options (model, sandbox, worker, MCP servers), see NAC's documentation.

## For everything else

Threading, worksets, sandbox flags, MCP configuration, skills, AGENTS.md, and session storage all work the same as in NAC. See [NAC's README](https://github.com/arcee-ai/nac) for documentation on shared features.
