# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.4] - 2026-11-26

### Fixed

- **A provider URL is local only if it points at the loopback address.** The "no key needed" hint matched any address that merely contained `localhost`, so `https://localhost.example.com` was taken for a local one.
- **A folder picker that fails now says why** instead of doing nothing.
- **Connect no longer stays broken when agent-kit's `scripts/cli.mjs` disappears.** The built-in copy was checked only by its marker and `plugin.json`, so an antivirus that removed `cli.mjs` left every action failing with "Cannot find module" and no way to repair it from the window; the copy is now rebuilt.
- **A language switch no longer leaves the Projects page in the previous language.** agent-kit answers in the language it was called with; the check is repeated after a switch and the inspection and plan return with the next Check instead of showing two languages. The "?" buttons of headings that keep their text now follow the switch too.
- **A deep project path no longer makes the projects card scroll sideways.**
- **The VPN toggle no longer freezes the window while the core stops** (up to 10 s in TUN mode), and a failed core download reports the download instead of "the mihomo core was not downloaded".
- **After a Claude update the model aliases use the new rules from the first launch.** The Desktop rule list was read after the aliases were written, so the first start after an update could keep stale names.
- **The state line sees a Claude started in VM mode** (the profile there is the standard `%LOCALAPPDATA%\Claude-3p`, not the PGC one).
- **A substituted Claude is rejected.** The Authenticode check compared only the certificate's CN, so a code-signing certificate from a corporate proxy's root (or a revoked one) passed for `Claude-Setup.exe` and the portable `claude.exe`. Both must now be issued by Anthropic's DigiCert code-signing CA; the revocation check runs when the network allows it and is skipped behind a proxy instead of blocking the update (ADR-022).
- **The project checks can no longer pass silently.** `ci.ps1 -Changed` took a failed `git diff` for "only docs changed" and skipped fmt, clippy and the tests; `ui-shots.mjs` printed `CLIPPED`/`HSCROLL` findings and always exited 0; `check_i18n.js` did not look at `data-help` topics or at keys picked by a ternary. All three now fail, and the i18n check covers the `help.*` texts and both sides of `t(…)` ternaries.
- **"Connect project" can no longer hang forever.** A grandchild (`node` under `claude.cmd`) that keeps the pipe open after the command exits left the window waiting for EOF; the output is now read as it comes, capped at 8 MB per stream, and the wait after exit is 2 s, keeping what already arrived.
- **"Update the installed Claude" no longer fails with 403.** `claude.ai` now serves the Desktop setup link only to a browser User-Agent and answers `provider-gateway-for-claude/…` with `403 Forbidden`. That one request uses a browser User-Agent; the portable download from `downloads.claude.ai` is unchanged.
- **Host Cowork patches follow Claude 7.37095.0.** The `msix_required` check now calls its message formatter through a different minified name, so the old anchor found nothing and the whole host patch was skipped ("Cowork (host-loop) does not fit: `msix_required` found 0 places"). The anchor now takes the name as a group; every other anchor of the set matches 7.37095.0 unchanged.
- **Cowork on the host no longer shows "Claude Code setup failed (prepare: no_vm)".** At session start the main process starts the VM itself, and the earlier patch covered only the window's call, so outside MSIX that path answered the error while the session went on in the host loop. The internal start now leaves through the same "not supported" branch as a machine without the VM feature; the copy is rebuilt once.
- **The Artifacts tick now really turns artifacts off.** Clearing the checkbox removed nothing from the gateway entry, so `artifactsEnabled` and its two companions stayed `true` for good; the keys PGC wrote are now taken out again and Claude's own choice applies.
- **The "Through MSIX, with VM" hint no longer says the mode does not work.** It described the state before the standard-profile fix; it now says the real Claude from the package runs with the standard `Claude-3p` profile, as it does.

### Added

- **The translated copy can be built from the MSIX package.** Anthropic's Windows installer now ships Claude as an MSIX package and removes the Squirrel folder, so `Claude Desktop not found` would appear again once the portable nupkg feed goes away. When no explicit path, Squirrel folder or `Programs\claude` is found, PGC now takes `claude.exe` from the installed package (found through the per-user `AppModel\Repository\Packages` registry key, no PowerShell: 33 ms instead of 1.3 s for `Get-AppxPackage`). The copy folder version comes from the package name (`Claude_6.28899.6.0_x64__…` → `app-6.28899.6.0`); registry repair and the upkeep checks accept `<package>\app` as a version folder. Priority is unchanged, so a configured portable Claude is not displaced by an older package. Checked live on the real package: a copy was built and patched in 25 s (ADR-021). Not checked: launching that copy.

- **Cowork in Claude Desktop on 3P, without an MSIX install.** In 3P Claude runs Cowork sessions on the host, yet its `msix_required` check turns Cowork off for any copy not started from MSIX. The Russian copy now carries small `app.asar` patches that skip that check, the VM capability and Windows-feature checks and the VM image download (over 1 GB), so Cowork opens on the portable copy with no administrator rights and no VM; the gateway profile gets `desktopHome: "standard"`. Checked in Windows Sandbox: a message starts a Cowork session that runs Claude Code on the host. File tools, web and MCP work. Cowork's own shell tool lives in the VM, so on the host the session gets Claude Code's PowerShell and Bash instead: not isolated, like the Code tab on Windows, and every call asks for approval (checked: a file-writing command waited, then ran and wrote the file). Advanced file analysis stays unavailable (ADR-018).
- **Cowork with the VM, through MSIX, as an option.** The new `Cowork` setting offers `On the host, no VM` (the default, portable), `Off` (Chat only) and `Through MSIX, with VM`. The last one starts the real `claude.exe` from an installed Claude MSIX package with the standard `%LOCALAPPDATA%\Claude-3p` profile (the VM service keeps its image there and refuses a junction), because the VM service accepts only a client started from the package folder (a translated copy with the package identity is refused: checked). PGC writes its gateway entry into that profile's `configLibrary` without deleting other entries, so keys, models, artifacts, search and Code work; there is no Russian translation in this mode (the package's asar is signed and read-only). Without the MSIX or its service PGC falls back to the host mode and says so. Checked on a machine with the MSIX and HCS: with the standard profile the service configures, the VM boots in about 14 seconds and a shell command runs inside the VM.
- **A compaction window setting that works for models marked `[1m]`.** The new `Conversation compaction window` field (Claude Code group of the settings) takes `400k`, `400000` or `40%` of the model's window, at least 100k. Claude Code 2.1.330 reads the window of a `[1m]` model from the settings key with `[1m]` but `/autocompact` writes the key without it, so the value was ignored ("a higher-priority override is active (1m tokens)"). PGC writes both keys under `modelSettings` in `~/.claude/settings.json` for every gateway model (touching only what it wrote when the field is cleared) and gives its console `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, which wins over the file. ADR-019. Not verified: the actual compaction in a Desktop session at the set threshold, only what `/autocompact` reports.
- **Artifacts (Design and Slides, beta).** A settings tick writes `artifactsEnabled`, `artifactsDesignEnabled` and `artifactsSlidesEnabled` to the 3P profile; live artifacts work without it.

### Changed

- **Claude opens on Chat, not in the Code tab.** The window's surface comes from `preferences.sidebarMode` in the profile's `claude_desktop_config.json`; one click on Code left `epitaxy` there, and every launch from PGC then opened Code, so Chat seemed broken. PGC now switches a stored Code surface (`code`, `epitaxy`) to `chat` when it writes the profile; an explicit Chat choice and other values are left alone (ADR-020).
- **The 3P profile now sets `desktopHome` and projects explicitly.** `desktopHome: "standard"` (or `"no-vm"` when Cowork is off) and `projectsEnabled: true` no longer depend on Claude's date-based defaults; existing user keys are still merged.

## [0.2.3] - 2026-11-25

### Fixed

- **The effort menu of the Russian Claude is in Russian.** Effort levels, their description and the thinking modes come from the model catalog, and Claude translates them from a second language file (`i18n/dynamic/<language>.json`) that has no Russian version, so the menu stayed in English. The copy now carries `i18n/dynamic/ru.json` with those 51 strings, in claude.ai's own wording; the copy is rebuilt once. The file keeps up with Claude updates (ADR-009): PGC also downloads claude.ai's `dynamic/ru.json` (`claude-ru-dynamic.json` in the settings folder, once per Claude version, best effort) and builds the menu file for the keys of the installed Claude's `dynamic/en-US.json`, taking each string from the download, then the bundled dictionary, then the main translation of the same English text. A new download rebuilds the copy. The settings check shows "effort menu N of M".

- **Claude Code works with an Anthropic API key, in the Code tab of Claude Desktop and in the console.** Anthropic answers an API key "Your credit balance is too low" whatever the balance when the request is labelled as interactive Claude Code (`cli`, the Code tab's `claude-desktop-3p`) in the `user-agent` or in Claude Code's attribution block at the top of the system prompt, so every message failed with "Billing problem". The gateway now introduces Claude Code to Anthropic-compatible providers as `claude -p` does (`sdk-cli`) in both places.

- **A gateway model no longer stays in Claude Code's own settings.** `/model` with Enter in a PGC console saves the choice to the user's `~/.claude/settings.json` as the default for new sessions, so a plain `claude` later opened on a model that exists only through the gateway ("Model … isn't recognized"). PGC now remembers the user's own model when it opens a console and, on exit and on the next start, puts it back if a gateway model took its place (or removes the key if there was none). Only `model` changes, and only when it is a PGC alias or a provider's model ID other than `claude-…`, and the file does not send `claude` to the person's own endpoint (`env.ANTHROPIC_BASE_URL`); an unreadable file is left alone (ADR-010).

- **One row per 1M model in Claude's model menu.** A model marked `[1m]` showed twice: a standard row (Claude counts 200K) and "… 1M" with an English "1M context window" note, though the provider has one model behind both. PGC now asks Claude for the 1M option only (`prefer1m`). Claude models from Anthropic get one row too: Sonnet, Opus and Haiku 5.5 have a native 1M window without a header, so the two rows behaved the same (ADR-011). Haiku 5.5 costs more above 100K tokens whichever row is picked. A non-Claude model without `[1m]` and without reasoning levels now counts 1M too, like every other model of unknown window, instead of Claude Desktop's own 200K.

### Added

- **Projects: connect and disconnect.** A new sidebar page connects agent-kit (checks, a critic and secret guarding for agents) to a project without typing commands. **Check** changes nothing and names what is missing: Node 22+, git, Claude Code, a git repository, where to get agent-kit, no critic chosen, uncommitted edits. **Connect** turns the plugin on only for you (`.claude/settings.local.json`, outside git) unless "for the whole team" is ticked, and updates an old copy. Then it shows the inspection and a plan with ticks; only ticked items are applied, and existing files are not replaced (`.gitignore` and the Claude settings get lines added). Every case has a test, and a test fails when a case of the table has none (ADR-014).

- **Disconnect a project** (ADR-017). For a connected project the page offers "Disconnect…": a plan with ticks of what agent-kit added and nobody changed since (from the journal agent-kit keeps next to git, or a byte-for-byte match with its template), and the lines it added to `.gitignore` and the Claude settings. A file the person edited is listed and left alone. The plugin is taken out of the project last: uninstalled from this project (the local scope, where Connect put it), or turned off here when it is installed for all projects; "for the whole team" is a separate, unticked item. An older agent-kit without undo offers only the plugin item.

- **Projects is marked beta** in the sidebar and on its page: it is new, and its texts and steps may still change.
- **Project checks with ticks.** "Projects" lists the checks the project already writes down (package.json scripts, Cargo, Makefile, pyproject, Go, its CI), each with the file and line it comes from. The likely checks are ticked; commands whose name says they change or ship something (`fix`, `update`, `deploy`, `publish`…) are shown unticked with the reason, and a person may tick them. A project's own check script (`tools/verify.*`, `scripts/check.*` or the one its pre-push hook runs) is found and ticked, the root checks it already runs are unticked with that reason, and the new `verify.json` gets a rule per folder: a sub-project's checks run only when its own files change. The ticked ones become the steps of a new `.claude/verify.json`; a project's own `verify.json` is never changed: checks it already runs are shown unticked, and the ticked steps are shown as ready text to add by hand. PGC carries agent-kit `b6afdc8d58df`, which gives this list. The page shows agent-kit's own texts (what the inspection found, the plan, the reasons) in the window's language, Russian or English. Nothing is run and no file leaves the computer: on 15 open projects this pick by code found 44 of the 59 checks their CI runs (with 54 extra picks, so the list is for a person to tick), about as many as a model did, so no model is asked (ADR-016).

- **What agent-kit learned, and a draft of critic rules.** Next to a check the page shows how many times it caught an error in this project (it failed, then passed after a fix), and advises about a long step that never failed; nothing is removed by itself. When the project has no `.claude/critic.md`, the plan offers one made from the project's own rules (README, AGENTS.md, ADRs) by code: unticked, with the rules shown in full. A model is not asked: on the bench its draft made the critic reject clean changes.

- **“?” explanations in plain words.** Next to the project page, its sections, the models list, the reasoning level, the critic, the proxy and Claude Code there is a small “?”: it opens a window that says what the thing is, why it is there and what happens if you leave it alone, in the window's language and theme. While something runs, buttons and lines show the brand's work mark.

- **agent-kit comes inside PGC.** The installer and the portable exe carry a pinned agent-kit snapshot (ADR-015). Connecting a project with no agent-kit marketplace unpacks it to PGC's folder and installs it from there, with a real version, so a newer PGC can offer "Update agent-kit" on the page; nothing updates by itself. A marketplace already pointing elsewhere is left alone. Node 22 or newer is still needed for agent-kit itself.

- **A critic model for project scripts.** Settings → Claude Code has a "Critic for project scripts" choice: another gateway model that reviews an agent's work (for example agent-kit's critic), so people without a Claude subscription get a reviewer too. When it is chosen and the gateway runs, Claude Desktop and the console opened from PGC get `AGENT_KIT_PGC_URL`, `AGENT_KIT_PGC_TOKEN`, `AGENT_KIT_PGC_MODEL` and `AGENT_KIT_CRITIC_VIA=pgc`; changing it does not restart the gateway (ADR-012).

## [0.2.2] - 2026-11-24

### Added

- **MCP tools on demand (tool search).** Through a gateway, Claude Code used to send the schema of every MCP tool on every turn: with a few connectors that was 500–600 tools and 150–180 thousand tokens before a word was said. Claude Desktop now gets `toolSearchEnabled` and the console `ENABLE_TOOL_SEARCH=true`, so Claude Code finds tools with its own `ToolSearch` as the regular Claude does; on the OpenAI path the gateway, like Ollama, keeps a tool marked `defer_loading` from the provider until `ToolSearch` loads it or the conversation has already called it. Seen live: 41.7 thousand input tokens on the first turn instead of about 220 thousand, in the Code tab and in the console; Anthropic-compatible providers (DeepSeek and GLM behind OpenCode Go, tried) take the deferred tools as they are.
- **A new app icon** in the style of the window: an iron tile with the gateway's bridge mark, each size drawn on its own, and a separate tray icon. After an update the installer tells Explorer, so the taskbar, Start and the desktop show the new icon at once instead of the cached old one.
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

- **Free web search for Claude's built-in `WebSearch`.** Claude's `WebSearch` (Claude Code and Claude Desktop) makes a nested request whose only tool is Anthropic's server tool `web_search_20250305`; third-party providers cannot answer it, so the model got nothing. The gateway now answers that request itself, without calling the provider: `server_tool_use`, `web_search_tool_result` and a short list of titles, links and snippets, streamed with `ping`s while the search runs. `allowed_domains` / `blocked_domains` filter the results by host (a single allowed domain is also searched as `site:`; with several, the list may come back empty); a blocked search or an empty result reaches Claude the way Anthropic's server reports it. No API keys: Brave first, DuckDuckGo HTML and lite as fallbacks. Answers are cached for 10 minutes; a source that shows a bot check rests for 5 minutes. `POST /search` (plain `{"q"}` → `{results}`) stays as a helper, closed to requests from web pages. When a search engine asks for a human check, the page is opened in the app's own WebView2 as a real visit, so the check is passed (the window shows itself after 8 s if it needs a tick). The browser pass has one 60 s deadline per question.
- **Built-in browser switch in Settings** ("Claude Desktop built-in browser"): third-party Claude turns its built-in browser off by default; the app now writes `builtinBrowserEnabled` into the 3p configuration entry, so it can be switched on from the gateway. Default off.

- **Window redesign: one style, crisper text, your own theme and language.**
  - Settings are grouped (Claude Desktop, Claude Code, App) with shorter hints; "Save settings" is active only when something changed and says what applies when.
  - A status strip in the footer shows whether Claude is open in the gateway's profile and whether it still runs with the current settings ("restart needed"); a failed launch stays there instead of vanishing with the toast. Toasts sit above the footer and no longer cover the title bar or the card's buttons; "Stopped" is red.
  - One scale for the whole window: three corner radii (5 / 9 / 14 px), five whole-pixel type sizes (12 / 13 / 14 / 16 / 20 px), controls 34 px high (small ones 26 px). Text is drawn on the pixel grid: line heights in pixels, no blurred translucent bars, disabled buttons dimmed by colour instead of opacity (both made ClearType fall back to grey smoothing). Monospace is Cascadia Mono / Consolas.
  - Light and dark theme can be chosen in the app (title-bar button and Settings, "Same as Windows" is still the default), and so can the language (RU / EN button next to it). The choice is remembered and painted before the first frame.
  - The light theme is neutral grey instead of cream; placeholders and accent text pass WCAG AA; no coloured glow or gradient on buttons. The window passes the Impeccable detector in both themes (`node tools/ui-shots.mjs --detect`); `node tools/ui-shots.mjs` takes the snapshot set (light / dark, 1920 / 1280 / 1100 / 1000 / 900 px and Windows display scales x1.25 / x1.5, empty / error / loading / long text) into `state/shots/` with measurements.

### Changed

- **The Claude Code console shows the model's real name** (`deepseek-v4.1-flash` instead of the alias `claude-<provider>-<digits>` that Claude Desktop needs), in its header and in the Default row of `/model`, as `ollama launch claude` does. The alias stays only when two gateway models share one ID.
- **Code-tab sessions of Claude Desktop get the same Claude Code settings as the console**: task tools, auto mode without the server check, no claude.ai connectors, tools without inline blocks, tool search. The 3p entry also allows every host for Cowork's network access and turns both telemetry switches off, as Ollama writes them.

### Removed

- **Strong and fast models for Claude Code, the advisor switch and the copied command.** Settings keep one model for Claude Code, the main one. In the console every role goes to it (`opus`, `haiku`, `fable`), and each row of `/model` shows it by name (`ANTHROPIC_DEFAULT_*_MODEL_NAME`) instead of Anthropic's Opus and Fable with their prices, which are not reachable through the gateway. Gone with them: the "Strong is the advisor (/advisor)" box, which wrote `advisorModel` into Claude Code's shared `settings.json`, the advisor that the gateway ran on the strong model, and "Copy the Claude Code command". The app no longer touches Claude Code's settings file; an `advisorModel` written there earlier stays until `/advisor off`. Roles saved by 0.2.1 are read and dropped. The gateway now drops an advisor declaration and the advisor blocks of older sessions from every request, for both provider types, so a leftover `advisorModel` or a resumed session does not reach a provider that cannot run it (a request whose only tool was the advisor goes without `tools`).

### Fixed

- **The context window of a model the catalog does not know is 1M again.** A smaller assumed window let 600 MCP tools fill it, and a Code-tab session compacted in a loop with nothing left to compact. A window that is too large is safe: an overflow comes back as Anthropic's "prompt is too long" and Claude Code compacts. When the catalog knows a window under 1M, the console gets it and `CLAUDE_CODE_DISABLE_1M_CONTEXT=1`, so the Default row of `/model` no longer claims 1M.
- **Processes that Claude starts see the real `AppData\Local` in English too.** Claude runs in its own profile with `LOCALAPPDATA` pointing at the app's folder; only the copy of Claude (with its child-environment fix) gave Code sessions, their npm, uv, gh, git and MCP servers the real folder. Without Russian the original Claude was started, and those tools wrote their caches into `%LOCALAPPDATA%\ClaudeProviderGateway`. The copy is now built without Russian as well, with no translation (about 700 MB on disk); if this Claude version cannot take the fix, the original opens and the window says so.
- **Stability after the review against Anthropic's gateway docs:**
  - errors in Anthropic's own types: `402` billing, `503` → `529` overloaded, `504` timeout, an exhausted quota → `402`; a context overflow worded by an Anthropic-type provider becomes `prompt is too long`, so Claude Code compacts;
  - an OpenAI stream that ends without `finish_reason` or `[DONE]` is an error, not a finished answer;
  - passthrough relays `x-should-retry`, `request-id` and `anthropic-ratelimit-*`; a provider's own headers replace Claude's (an empty value drops one);
  - auto mode works without the server check (`inferenceGatewayAutoModeServerChecks: false`, `CLAUDE_CODE_AUTO_MODE_SERVER=0`);
  - the Claude Code console gets `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`, `CLAUDE_CODE_ENABLE_TODO_TOOLS`, `CLAUDE_CODE_INLINE_TOOLS=false`, the main model's window and output limit from the catalog;
  - the app: a startup race of the VPN core, a reserved port (`WSAEACCES`), the core restarted after a crash, its log at `warning`, registry repair on any exit and after Claude restarts itself, the copy not built from a half-installed Claude, old copies removed, window commands off the main thread.
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

- **Two gateway builds no longer fight over one Claude profile.** The release build, the dev build (identifier `.dev`) and an isolated run (`CLAUDE_GATEWAY_HOME`) each get their own Claude folder (`ClaudeProviderGateway`, `ClaudeProviderGateway-dev`, `<home>\claude`), so they no longer overwrite each other's configuration entry (port, token) or share the profile lock. If the port is busy (another gateway on it), the gateway takes a free one instead of failing, and keeps it across restarts.
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

[Unreleased]: https://github.com/d-ridelman/provider-gateway-for-claude/compare/v0.2.4...HEAD
[0.2.4]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.4
[0.2.3]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.3
[0.2.2]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.2
[0.2.1]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.1
[0.2.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.2.0
[0.1.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.1.0
