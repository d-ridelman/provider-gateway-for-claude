# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Free web search for Claude's built-in `WebSearch` (dev branch `dev/search`).** Claude's `WebSearch` (Claude Code and Claude Desktop) makes a nested request whose only tool is Anthropic's server tool `web_search_20250305`; third-party providers cannot answer it, so the model got nothing. The gateway now answers that request itself, without calling the provider: `server_tool_use`, `web_search_tool_result` and a short list of titles, links and snippets, streamed with `ping`s while the search runs. `allowed_domains` / `blocked_domains` filter the results by host (a single allowed domain is also searched as `site:`); a blocked search or an empty result reaches Claude the way Anthropic's server reports it. No API keys: Brave first, DuckDuckGo HTML and lite as fallbacks. Answers are cached for 10 minutes; a source that shows a bot check rests for 5 minutes. `POST /search` (plain `{"q"}` → `{results}`) stays as a helper, closed to requests from web pages. When a search engine asks for a human check, the page is opened in the app's own WebView2 as a real visit, so the check is passed (the window shows itself after 8 s if it needs a tick). The browser pass has one 60 s deadline per question. Details and measurements: `docs/websearch-work.md`.
- **Built-in browser switch in Settings** ("Claude Desktop built-in browser"): third-party Claude turns its built-in browser off by default; the app now writes `builtinBrowserEnabled` into the 3p configuration entry, so it can be switched on from the gateway. Default off.
- **The full list of 3p settings** (222 keys in 8 categories, from the decompiled app) is in `docs/claude-3p-settings.md`, with the ones worth surfacing next (auto mode, browser domains, model pricing, default effort).

- **Advisor switch in Settings** ("Strong is the advisor (/advisor)"): the app writes `advisorModel` to Claude Code's shared settings (`~/.claude/settings.json`, or `CLAUDE_CONFIG_DIR`), so `/advisor` is no longer needed. One line changes, the rest of the file stays byte for byte; the first original is kept as `settings.json.bak`. Ticked by default until you decide, and the first save writes it once; after that the app never turns it on by itself. A model already chosen with `/advisor` is kept; an unreadable settings file is left alone, and the other settings are saved anyway. The tooltip says what it affects (the regular Claude Code too).

### Fixed

- **Picky models no longer break the session.** `glm-5.3` (unlike `glm-5.3-flash` and DeepSeek) answers any picture with a bare 400 "the request is invalid", and once a screenshot was in the conversation every later request failed, whatever you retried. The gateway now handles this class of trouble:
  - on an unexplained 400 it retries the request simplified, one step at a time: images replaced by a note, then the reasoning effort, the sampling parameters (`temperature`, `top_p`, `stop`, `parallel_tool_calls`) and `stream_options` dropped one by one, tool names shortened to the 64-character `[A-Za-z0-9_-]` form (restored in the answer), consecutive messages merged, tool schemas simplified, tools capped at 128;
  - the single step that made the difference, confirmed on two different requests, (for pictures: on two different pictures, since the history repeats the same one every turn) is remembered in `model-profiles.json` next to the settings (expires after 30 days; delete to forget), so the next requests are simplified up front;
  - a conversation that no longer fits the model's window is reported to Claude Code as Anthropic's "prompt is too long", so it compacts and retries by itself; provider errors keep their kind (rate limit, overloaded, authentication…), so Claude Code retries what it should;
  - repeated tool names, which providers refuse, are sent once;
  - the gateway log now carries what the provider answered to every refusal, and what the gateway did about it.
- **Context compaction kicked in far too early with screenshots.** The gateway counted an image by its base64 text: one 1920×1140 screenshot came out as ~250 thousand tokens instead of ~1.5 thousand, so Claude Code saw the 1M window as full and compacted the conversation again and again. Images are now counted by their pixel size, as Claude counts them (PNG, JPEG, GIF, WebP), PDFs by pages.
- When the provider reports usage in the stream, its real input size (`prompt_tokens`) now reaches Claude Code at the end of the reply, instead of the gateway's estimate; cached tokens are no longer counted twice.

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

[0.2.1]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.1
[0.2.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.0
[0.1.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.1.0
