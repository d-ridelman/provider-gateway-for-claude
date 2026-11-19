# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [0.2.2] - 2026-11-19

### Fixed

- The advisor failed with "Advisor unavailable" in long sessions: the strong model was asked for its whole answer at once, and on a big conversation the connection broke while it was thinking. It is now asked with streaming, like the main model. The gateway log shows the full cause of a network error.

## [0.2.1] - 2026-11-19

### Added

- **Advisor through the gateway.** Claude Code's advisor (`/advisor`) is a server-side tool at Anthropic and was off through third-party providers. With a strong model chosen, the gateway now runs it: the main model calls `advisor`, the gateway asks the strong model with the conversation and continues the same reply with Anthropic's `server_tool_use` / `advisor_tool_result` blocks. Works with OpenAI-compatible providers in the Claude Code console and in the Code tab of the Russian copy of Claude Desktop. Any advisor model Claude Code names that the gateway does not know (`claude-fable-…`, `claude-opus-…`) is answered by the strong model.
- Subagents with `model: fable` use the strong model (in the console; in the Russian copy when the strong model is on an OpenAI-compatible provider).

### Changed

- The Russian copy lifts Claude Desktop's ban on experimental betas for Code sessions when the advisor is on (the advisor is one of them), so these sessions behave like Claude Code in the console.

## [0.2.0] - 2026-11-19

### Added

- **Models for Claude Code** in Settings: a gateway model for each role Claude Code asks for — main (`sonnet`; also the gateway's model for unknown names, so unassigned roles and inheriting subagents such as Explore land on it), strong (`opus`) and fast (`haiku`). Applies to the Claude Code console, the copied command and the Code tab of Claude Desktop (`anthropicFamilyTier` in its model list).

### Fixed

- Code sessions in the Russian copy of Claude (and their shells, MCP servers and other tools) now see the usual `%LOCALAPPDATA%` like in the regular Claude, instead of the app's own folder; running `claude.exe` from a session no longer pops up in the copy.
- Requests no longer fail with 400 "is not a regex" when an MCP tool schema has a pattern the provider's regex engine (Go/RE2) does not accept; such hints are dropped from the schema.

### Changed

- **Copy the Claude Code command** is built from the same environment as the Claude Code button (it now includes the context window and all model roles).
- The console's context window follows the main model (1M only when it supports 1M).

## [0.1.0] - 2026-11-19

The first public release.

### Added

- Local gateway on `127.0.0.1` with the Anthropic Messages API: translates to OpenAI Chat Completions (streaming, tool calls, images, reasoning effort) or passes through to Anthropic-compatible providers.
- Windows app (Tauri 2): providers with their own key, models and reasoning ceiling; templates for OpenAI, OpenRouter, OpenCode Go and Zen, DeepSeek, Gemini, Anthropic, Ollama, llama.cpp and LM Studio.
- Model list from the provider, with names, 1M-context models and reasoning levels from the OpenCode model catalog.
- **Test connection**: one real request through the gateway.
- Claude Desktop in a separate profile; the user's own Claude account and chats are not touched.
- **Claude Code** button: a console in the project folder with `claude` already connected to the gateway.
- Compatibility self-check against the installed Claude Desktop (model rules, config fields, translation coverage).
- Optional Russian interface for Claude Desktop in a separate copy, with the official claude.ai translation; the copy also offers **No folder** for Code sessions, hidden in third-party mode, and is prepared in the background after a Claude update. When claude.ai answers programs with a bot check, the translation is loaded through the built-in browser.
- Interface in English and Russian, picked from the Windows display language; light and dark themes.
- Tray: closing the window keeps the gateway running.
- Updates from GitHub Releases, verified with the signing key built into the app.
- Provider keys in Windows Credential Manager; the gateway requires a local token.

[0.2.2]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.2
[0.2.1]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.1
[0.2.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.0
[0.1.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.1.0
