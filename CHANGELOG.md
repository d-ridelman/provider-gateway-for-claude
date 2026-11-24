# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Proxy and VPN on the mihomo core.** A Proxy and VPN page:
  - VPN subscriptions (Clash/mihomo or base64 v2ray links) as the core's proxy providers, each with a name prefix so equal node names of two subscriptions stay apart;
  - node tiles with latency, Auto (the fastest) first; traffic and expiry of a subscription;
  - Update and Remove for each subscription, and a title-bar chip that turns red when the core or the system proxy fails.
  
  Modes:
  - **Claude and providers only**: the gateway's clients, Claude from PGC through `--proxy-server` (seen live: its requests come as `CONNECT claude.ai:443`), the Claude Code console through `HTTPS_PROXY`;
  - **System proxy**: through WinINet, the previous settings including a PAC script are saved and restored on exit and at the next start, a group policy that locks the proxy is reported;
  - **All traffic (TUN)**: the core elevated through UAC under a helper instance of PGC that stops it with PGC.
  
  Private networks and loopback always go direct. A failed core leaves the gateway on the proxy port, so requests fail instead of going around the VPN.
  
  The core itself:
  - pinned (v1.19.34), downloaded from its GitHub releases on request with its sha256 built in, never bundled (GPL-3.0, a separate process);
  - runs in a kill-on-close job object; subscription addresses stay in Credential Manager and are cut down to their host in errors.
- **Claude Desktop and Claude Code downloads.**
  - Portable (nothing installed): Claude Desktop is unpacked from its full nupkg, its registry traces are removed after it closes; Claude Code gets its own `CLAUDE_CONFIG_DIR` and no self-update.
  - Regular install: the official installer, or `claude install`. Over an existing Claude the window asks first.
  - Checks: sha256 from the Claude Code manifest, sha1 from `RELEASES`, and an Authenticode signature by exactly "Anthropic, PBC" on every `claude.exe`.
  - An update keeps a portable Claude that is still open whole, because its folder is renamed before it is removed.
- **Claude downloads go through the VPN when it is on.** The downloader uses the core's mixed port; the installers get `HTTPS_PROXY`. A VPN that is on with no core running stops the download with a plain message. The core itself is always downloaded directly.
- **System proxy and TUN checked live in Windows Sandbox** (ignored tests `live_system_proxy_in_sandbox`, `live_tun_in_sandbox`, run only there).
  - System proxy: from a domain-like setup (PAC and auto-detect), turning it on writes the effective proxy, and a program using the system proxy came through ours; turning it off brought back the flags (auto-detect included), the PAC address and the bypass list exactly.
  - TUN: the core started elevated through the helper; a request without any proxy went through the TUN adapter by name and was answered; nothing was left running after the stop.
  - The live run found a bug, now fixed. With `respect-rules` the direct outbound resolved names with the core's own fake-ip DNS and dialled `198.18.x`. TUN DNS now goes without it, and the TLS/HTTP sniffer is on.
- **Hardening after review.**
  - Every core start checks the sha256 of `mihomo.exe` itself and keeps the file locked against writes until it runs (the elevated TUN helper included).
  - A refused UAC prompt puts the previous mode back.
  - The system proxy is restored on any exit (`RunEvent::Exit`), before an update installs, when the core crashes, and only if it is still ours.
  - The mode is read under the core's lock, so the last saved one wins; the mode radios are locked while one applies.
  - A port change at start restarts the gateway; the proxy port comes from a fixed range next to the gateway's.
  - Local names go direct, and the user's own `NO_PROXY` is kept.
  - The core's log tail is redacted, the core gets no browser-endpoint secret, and the node list of a removed subscription is deleted.
  - The window skips redraws when nothing changed and polls nothing while hidden.

- **Free web search for Claude's built-in `WebSearch` (dev branch `dev/search`).** Claude's `WebSearch` (Claude Code and Claude Desktop) makes a nested request whose only tool is Anthropic's server tool `web_search_20250305`; third-party providers cannot answer it, so the model got nothing. The gateway now answers that request itself, without calling the provider: `server_tool_use`, `web_search_tool_result` and a short list of titles, links and snippets, streamed with `ping`s while the search runs. `allowed_domains` / `blocked_domains` filter the results by host (a single allowed domain is also searched as `site:`; with several, the list may come back empty); a blocked search or an empty result reaches Claude the way Anthropic's server reports it. No API keys: Brave first, DuckDuckGo HTML and lite as fallbacks. Answers are cached for 10 minutes; a source that shows a bot check rests for 5 minutes. `POST /search` (plain `{"q"}` → `{results}`) stays as a helper, closed to requests from web pages. When a search engine asks for a human check, the page is opened in the app's own WebView2 as a real visit, so the check is passed (the window shows itself after 8 s if it needs a tick). The browser pass has one 60 s deadline per question. Details and measurements: `docs/websearch-work.md`.
- **Built-in browser switch in Settings** ("Claude Desktop built-in browser"): third-party Claude turns its built-in browser off by default; the app now writes `builtinBrowserEnabled` into the 3p configuration entry, so it can be switched on from the gateway. Default off.
- **The full list of 3p settings** (222 keys in 8 categories, from the decompiled app) is in `docs/claude-3p-settings.md`, with the ones worth surfacing next (auto mode, browser domains, model pricing, default effort).

- **Window redesign: one style, crisper text, your own theme and language.**
  - Settings are grouped (Claude Desktop, Claude Code, App) with shorter hints; "Save settings" is active only when something changed and says what applies when.
  - A status strip in the footer shows whether Claude is open in the gateway's profile and whether it still runs with the current settings ("restart needed"); a failed launch stays there instead of vanishing with the toast. Toasts sit above the footer and no longer cover the title bar or the card's buttons; "Stopped" is red.
  - One scale for the whole window: three corner radii (5 / 9 / 14 px), five whole-pixel type sizes (12 / 13 / 14 / 16 / 20 px), controls 34 px high (small ones 26 px). Text is drawn on the pixel grid: line heights in pixels, no blurred translucent bars, disabled buttons dimmed by colour instead of opacity (both made ClearType fall back to grey smoothing). Monospace is Cascadia Mono / Consolas.
  - Light and dark theme can be chosen in the app (title-bar button and Settings, "Same as Windows" is still the default), and so can the language (RU / EN button next to it). The choice is remembered and painted before the first frame.
  - The light theme is neutral grey instead of cream; placeholders and accent text pass WCAG AA; no coloured glow or gradient on buttons. The window passes the Impeccable detector in both themes (`node tools/ui-shots.mjs --detect`); `node tools/ui-shots.mjs` takes the snapshot set (light / dark, 1920 / 1280 / 1100 / 1000 / 900 px and Windows display scales x1.25 / x1.5, empty / error / loading / long text) into `state/shots/` with measurements.

### Removed

- **Strong and fast models for Claude Code, the advisor switch and the copied command.** Settings keep one model for Claude Code, the main one. In the console every role goes to it (`opus`, `haiku`, `fable`), and each row of `/model` shows it by name (`ANTHROPIC_DEFAULT_*_MODEL_NAME`) instead of Anthropic's Opus and Fable with their prices, which are not reachable through the gateway. Gone with them: the "Strong is the advisor (/advisor)" box, which wrote `advisorModel` into Claude Code's shared `settings.json`, the advisor that the gateway ran on the strong model, and "Copy the Claude Code command". The app no longer touches Claude Code's settings file; an `advisorModel` written there earlier stays until `/advisor off`. Roles saved by 0.2.1 are read and dropped. The gateway now drops an advisor declaration and the advisor blocks of older sessions from every request, for both provider types, so a leftover `advisorModel` or a resumed session does not reach a provider that cannot run it (a request whose only tool was the advisor goes without `tools`).

### Fixed

- **"Open in Claude Code" in the folder menu no longer disappears after Open Claude.**
  - Claude started with the PGC profile deletes the installed Claude's `Directory\shell\ClaudeCode` keys as a whole, because the feature is off in that profile (seen live with 6.28899.6), and they stayed gone until the regular Claude restarted.
  - Every launch now takes the values of those keys and of `claude://`, and puts back whatever disappeared, after the start and after Claude closes.

- **DeepSeek in thinking mode with tools.** DeepSeek's API wants the reasoning of earlier replies (`reasoning_content`) back whenever the request carries `tools`, and answers 400 without it. Claude Code sends the `thinking` blocks back in the history; the gateway now passes them on as `reasoning_content`, and leaves them out when the request has no tools. A provider that does not know the field answers a bare 400: the first rung of the retry ladder takes it out and the model is remembered. Not tried against the official DeepSeek API (no key here): the rule is from its documentation.
- **A stream broken by an error** (network, invalid JSON from the provider, a failed continuation) now ends with the `error` event; it used to be followed by a normal `message_delta` / `message_stop`, which passed a cut answer off as a finished one.
- **The gateway checks the token before it reads the body** of `/v1/messages` and `/count_tokens` (a stranger's huge POST used to land in memory first and only then got its 401); the oauth stubs and `/search` take bodies of 64 KB at most.
- **Counting the tokens of a long conversation** (~55 ms on 3 MB with screenshots, ~250 ms the first time) runs on the blocking pool, not on an async worker.
- **A quirk of a model is learned from separate requests only.** Subagents that ran at the same time and caught the same one-off refusal used to count as two confirmations and could switch a feature off for 30 days.
- **The build scripts** pass the path remaps through `CARGO_ENCODED_RUSTFLAGS`, so a profile or checkout path with a space no longer breaks the build.
- **A tool call cut by the token limit is no longer sent as `{}`.** When the provider ran out of tokens in the middle of a call (a long Write), the gateway sent the tool with empty arguments and `tool_use`: Claude Code ran it without a path, and the model repeated the same call. Now the cut call is dropped and the answer ends with `max_tokens`, so Claude Code raises the limit or asks to continue. A whole call next to a cut one stays (the answer then ends with `tool_use`, and the cut call is dropped).
- **Screenshots from tools reach the model.** A picture inside a tool result (a desktop-automation tool, Read of a PNG, computer use) became the word `[image]`. It now travels as an image message right after the tool messages; a model without vision goes down the usual ladder (the request is retried without the picture).
- **Tool calls without `index`** (some OpenAI-compatible providers send each call in a chunk of its own) are told apart by their `id` and name instead of melting into one call with `{}`.
- **Safer files.** `config.json` and the Claude profile files (`configLibrary`, `claude_desktop_config.json`) are written through a temporary file, so a cut write (full disk, power loss, antivirus) no longer leaves them empty; `config.json` is not rewritten at every start; a config that is not UTF-8 (Notepad's "Unicode") is set aside as `config.broken-*.json` instead of crashing the start. **A dev build now keeps its provider keys apart from the release's (Credential Manager service `claude-provider-gateway-dev`): enter them once again there.**
- **Web search.** A search source on pause (bot check, 429) is reported as `too_many_requests`, not as an empty result that made the model rewrite its queries for minutes; parallel searches no longer destroy each other's browser window; the internal browser endpoint needs a secret in its address and refuses requests that come from web pages; it is no longer left on a dead port after a restart and its address stays out of the log and out of Claude's environment.
- **`claude://` in the registry** is put back to the installed Claude when the Russian copy's profile is really free (it restarts itself, so the process we started is not the signal), and when PGC quits or updates.
- **Anthropic-type providers:** the key is cut out of error bodies (also in non-UTF-8 ones); a provider without `/count_tokens` gets the local estimate instead of a 404.
- **Release script:** takes the installer by its exact name (a `(dev)` build next to it used to win and the release failed), and for `-Publish` insists on a clean tree and a pushed `HEAD`.

- **Two gateway builds no longer fight over one Claude profile (dev branch `dev/search`).** The release build, the dev build (identifier `.dev`) and an isolated run (`CLAUDE_GATEWAY_HOME`) each get their own Claude folder (`ClaudeProviderGateway`, `ClaudeProviderGateway-dev`, `<home>\claude`), so they no longer overwrite each other's configuration entry (port, token) or share the profile lock. If the port is busy (another gateway on it), the gateway takes a free one instead of failing, and keeps it across restarts.
- **"Open Claude" no longer disturbs a Claude that is already open.** The app detects it (Claude holds `lockfile` in its profile while it runs) and does not start a second copy, rebuild the Russian copy or touch the configuration entry while its keys are unchanged: the existing window is shown. If the gateway's port, models or switches changed, it says so and asks you to close and reopen Claude; the app never closes it for you. The entry is no longer wiped at every launch: the gateway's own keys are written over it and the rest (what you changed in Claude's own settings window) survives. An unreadable entry is copied aside (`*.json.broken-<time>`) instead of being overwritten blindly.
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
