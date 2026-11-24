<p align="center">
  <img src="docs/images/icon.png" width="96" alt="">
</p>

<h1 align="center">Provider Gateway for Claude</h1>

<p align="center">
  Run Claude Desktop and Claude Code on the models you choose:<br>
  OpenAI, OpenRouter, DeepSeek, Gemini, OpenCode, Ollama, LM Studio or any OpenAI- or Anthropic-compatible API.
</p>

<p align="center">
  <a href="https://github.com/d-ridelman/provider-gateway-for-claude/releases/latest"><img src="https://img.shields.io/badge/download-Windows%20x64-c15f3c?style=flat-square" alt="Download for Windows"></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-3d3a35?style=flat-square" alt="Windows 10 and 11">
  <img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%202-3d3a35?style=flat-square" alt="Built with Rust and Tauri 2">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-3d3a35?style=flat-square" alt="License: MIT or Apache-2.0"></a>
</p>

<p align="center">
  <b>English</b> · <a href="README.ru.md">Русский</a>
</p>

<p align="center">
  <img src="docs/images/main-en.png" width="860" alt="Provider Gateway for Claude: a provider with its models, the gateway running">
</p>

> **Unofficial.** This project is not affiliated with, endorsed by or sponsored by Anthropic. "Claude" is a trademark of Anthropic, PBC; it is used here only to say which apps the gateway works with.

## What it does

Claude Desktop can run in third-party inference mode and Claude Code can talk to any Anthropic-style endpoint. Provider Gateway for Claude is a small Windows app that runs that endpoint for you on `127.0.0.1` and translates each request to your provider:

- **Anthropic Messages → OpenAI Chat Completions**, including streaming, tool calls, images (also the screenshots that tools return) and reasoning levels; or straight passthrough for Anthropic-compatible providers.
- **Claude Desktop in a separate profile**, so your own Claude account and chats stay untouched. Your models appear in Claude's model menu under readable names. If that Claude is already open, **Open Claude** only shows it (and says when the gateway settings changed); the app never closes or restarts it.
- **Claude Code in one click**: a console opens in your project folder with `claude` already connected to the gateway.

## Features

- Any number of providers, each with its own key, models and reasoning-level ceiling. Templates for OpenAI, OpenRouter, OpenCode Go and Zen, DeepSeek, Gemini, Anthropic, Ollama, llama.cpp and LM Studio.
- **Load models from the provider**, with hints from the open [OpenCode model catalog](https://models.dev): readable names, 1M-context models and supported reasoning levels are filled in for you.
- **Test connection** sends one real request through the gateway and shows the answer.
- **Free web search for Claude.** Claude's built-in `WebSearch` (Claude Code and Claude Desktop) works without any key or setup: the gateway answers it itself, see [Web search](#web-search).
- **VPN subscriptions on the mihomo core.** Pick a node or Auto, test latency and choose what goes through the VPN: Claude and providers only, the system proxy (works on domain PCs) or all traffic (TUN). See [Proxy and VPN](#proxy-and-vpn).
- **Claude without installing.** Claude Desktop and Claude Code download from Anthropic's own addresses, either portable with nothing installed or as a regular install. The signature and checksum are verified. See [Claude without installing](#claude-without-installing).
- **Keys stay in Windows Credential Manager**, never in files. The gateway listens on `127.0.0.1` only and requires a local token. The config files are written through a temporary file, so a cut write cannot empty them.
- **Adapts to picky models.** Providers differ in what they accept, and many answer with a bare 400. The gateway prevents the known trouble (repeated tool names, a conversation that no longer fits the window is reported to Claude Code the way Anthropic does, so it compacts by itself), and when a model refuses a request without saying why, it simplifies it step by step: images left out, reasoning effort, sampling parameters and stream options dropped, tool names shortened, consecutive messages merged, tool schemas simplified, tools capped at 128. The reasoning of earlier replies goes back to providers that require it (DeepSeek's thinking mode with tools) and is dropped for those that refuse it. The one step that made the difference, confirmed twice for a model, is remembered in `model-profiles.json` next to the settings (delete it to forget; entries expire after 30 days) and the gateway log says what it did and what the provider answered.
- **Keeps working after Claude updates**: the app reads Claude's model rules from the installed version, checks its config fields and tells you in Settings if something no longer matches.
- **Russian interface for Claude Desktop** (optional): a separate patched copy of Claude with claude.ai's own Russian translation. The copy also brings back **No folder** in the Code folder picker, which third-party mode hides. When Claude updates, the app prepares the new copy in the background, so **Open Claude** does not wait. Your installed Claude is not modified. The copy is built without Russian too (no translation): only in it do the processes Claude starts (Code sessions, their npm, uv, git, MCP servers) see the real `AppData\Local` instead of the app's folder. The copy itself does not update Claude; new versions come from the regular Claude.
- **Lives in the tray** while you work; the gateway keeps running when the window is closed.
- **Updates itself** from GitHub Releases; every update is verified against a signing key built into the app.
- **Built-in browser switch**: third-party mode turns Claude Desktop's browser pane off; a tick in Settings turns it on at every launch from the gateway.
- English and Russian interface, light and dark theme: as in Windows by default, and switchable in the title bar or in Settings.

## Requirements

- Windows 10 or 11, x64, with WebView2 (built into Windows 11).
- [Claude Desktop](https://claude.ai/download) for the desktop chat. Tested with Claude Desktop 6.28899.6.
- [Claude Code](https://docs.claude.com/en/docs/claude-code) for the terminal agent (optional).
- An account and API key with at least one provider, or a local model server.

## Install

1. Download `Provider-Gateway-for-Claude_<version>_x64-setup.exe` from [Releases](https://github.com/d-ridelman/provider-gateway-for-claude/releases).
2. Run it. It installs for the current user only and needs no administrator rights.

The installer is not code-signed yet, so Windows SmartScreen may warn about an unknown publisher: choose **More info → Run anyway**. You can compare the file with `SHA256SUMS.txt` from the same release:

```powershell
Get-FileHash .\Provider-Gateway-for-Claude_*_x64-setup.exe -Algorithm SHA256
```

A portable `.exe` is attached to each release as well. Its built-in update runs the installer, so after updating you get the installed app; the portable file itself stays as it was.

## Quick start

1. Click **+**, pick a template (for example *OpenRouter*), paste your API key.
2. Click **Load from provider** and pick models, or type model IDs one per line. Add `[1m]` to a model that has a 1M-token context window.
3. Click **Save**. The gateway starts on its own.
4. Click **Open Claude** for Claude Desktop, or **Claude Code** for a console with the CLI.

<p align="center">
  <img src="docs/images/new-en.png" width="49%" alt="A new provider: template, address, key and models">
  <img src="docs/images/settings-en.png" width="49%" alt="Settings: Claude Desktop, Claude Code with its main model, the app (language, theme, port, token, updates)">
</p>

### Reasoning levels

Set **Max reasoning level** on a provider and Claude offers a level picker for its models. The gateway passes the chosen level as `reasoning_effort` (OpenAI-compatible) or `output_config.effort` (Anthropic), capped at the maximum. **Default level** is used when Claude sends none, for example Claude Code without `--effort`.

### Main model for Claude Code

Settings → **Model for Claude Code** picks the gateway model for Claude Code. In the **Claude Code** console every role Claude Code asks for goes to it: the session, subagents that inherit the model (Explore, Plan), `model: opus`, `model: haiku` and `model: fable`, and every row of `/model` shows it by name instead of Anthropic's own Opus or Fable. In the Code tab of Claude Desktop (from the next **Open Claude**) it is the main (`sonnet`) model; the other roles there are Desktop's own choice. A subagent can also name any gateway model directly in its `model:` field, and `/model <name>` switches the session.

### Web search

Claude's `WebSearch` normally runs on Anthropic's servers, so through a third-party provider the model got nothing. The gateway now answers it itself, in the form Anthropic's server uses: it searches DuckDuckGo and Brave with no key (answers are cached for 10 minutes; the tool's `allowed_domains` and `blocked_domains` are honored). When a search engine asks for a human check, the app opens that page in its own WebView2 window as a real visit, and the window shows itself if it needs a tick. There is nothing to set up and no switch yet: the search words go to those search engines (see [Privacy and network](#privacy-and-network)). Checked in the Claude Code console and in the Code tab of Claude Desktop.

### Proxy and VPN

**Proxy and VPN** sits at the bottom left. The app runs the [mihomo](https://github.com/MetaCubeX/mihomo) core (Clash.Meta) with your VPN subscriptions. Paste a subscription address: Clash/mihomo links and base64 subscriptions with vless, vmess, ss, trojan and hysteria2 nodes all work. Its nodes appear as tiles with their latency. Click a node to use it, or pick **Auto** for the fastest. **Test latency** measures every node, and a subscription's row shows its traffic and expiry. The core updates subscriptions on its own every 6, 12 or 24 hours, and Update does it now. The title bar shows a "VPN · node · ms" chip; it turns red if the core stops or a policy resets the proxy.

What goes through the VPN:

| Mode | Through the VPN | Rights |
|---|---|---|
| Claude and providers only | the gateway (provider requests, web search, model catalog), Claude from PGC (`--proxy-server`) and the Claude Code console (`HTTPS_PROXY`) | none |
| System proxy | plus browsers and programs that use the Windows proxy (your regular Claude too) | none, so it suits domain PCs |
| All traffic (TUN) | every program | administrator: Windows asks each time the core starts |

Local addresses, private networks and `.local`, `.lan`, `.internal`, `.home.arpa` names (Ollama, LM Studio, the intranet) go direct in every mode. A LAN server named without a dot or by a corporate domain goes through the VPN, so give such a server by its IP. If the core is not running, the gateway's requests fail with an error instead of slipping past the VPN.

The core is not part of the app. **Download the core** gets a pinned version from its author's GitHub (v1.19.34 now, about 22 MB) and checks its sha256, which is built into PGC: a newer core arrives with a PGC update, signed with PGC's key. The core runs as a separate process and closes with PGC. Some antivirus programs flag mihomo as a "potentially unwanted program"; if the file is quarantined, the window says so and offers to download it again.

**System proxy** is set through WinINet, the way the Windows Proxy settings page does it. Your previous settings (manual proxy, PAC script, auto-detection, bypass list) are saved before the switch and come back when you turn the mode off or quit PGC. If PGC crashed, that happens at its next start; until then programs point at a dead port. If a group policy forbids changing the proxy, the window says so, and Claude and providers only still works. If a domain PC reaches the internet only through a corporate proxy, the core cannot yet reach the nodes through it.

**TUN** starts the core with administrator rights through UAC; PGC itself stays a normal process. An elevated helper stops the core when PGC closes. Before every start, the sha256 of `mihomo.exe` itself is checked, and the file stays locked against writes until the core starts, so it cannot be swapped in the user's folder. If the UAC prompt is refused, the previous mode stays. If the core crashes, the system proxy is put back on its own and the chip turns red.

A subscription address is a secret: it is kept in Credential Manager, and the window and the log show only its server. The core gets it in its own config file (`mihomo\home\config.yaml`) and keeps the downloaded node list in `mihomo\home\providers`.

### Claude without installing

Settings → **Download Claude Desktop** and **Download Claude Code**: both come from Anthropic's own addresses, in one of two ways.

- **Without installing** (for PCs where you may not install software). Claude Desktop is unpacked from its full package into the app folder, and Open Claude runs it. Claude Code goes into the same folder, and the "Portable Claude Code" tick makes the **Claude Code** button use it. Portable Claude Code has its own settings folder (`CLAUDE_CONFIG_DIR`) and its self-update is off; PGC updates it with the same button.
- **Install**: the official Claude Desktop installer, or `claude install` for Claude Code, the way Anthropic does it. If Claude is already installed, the window asks first, because that is your regular Claude.

With the VPN on in PGC, downloads go through it. If the core is not running, the download does not start and the window says why; if the node does not answer, the error suggests picking another. The mihomo core itself is downloaded directly.

Checks: sha256 from the release manifest for Claude Code, sha1 from `RELEASES` for Claude Desktop (the same check Squirrel makes). Every `claude.exe` must carry an Authenticode signature by "Anthropic, PBC". A portable Claude writes `claude://` into the registry when it starts; once it closes, PGC gives those keys back to the installed Claude, or removes them if there is none.

## Privacy and network

The app talks to:

| Where | When |
|---|---|
| The providers you add | When Claude or Claude Code sends a request, and on Load models / Test connection |
| `models.opencode.ai` | On **Load from provider**, for model names and limits; nothing about you is sent |
| `claude.ai/i18n/ru.json` | Only with the Russian Claude interface turned on, when Claude updates |
| `github.com` (this repository's releases) | Update check at start and once a day; turn it off in Settings |
| `html.duckduckgo.com`, `lite.duckduckgo.com`, `search.brave.com` | When Claude's built-in web search runs: the gateway answers it itself and sends the words the model chose to search for (no account, no keys). It is always on (there is no switch for it yet) |
| `github.com/MetaCubeX/mihomo` (releases) | Only on **Download the core** |
| Your VPN subscription addresses and their nodes | While the proxy is on: the core updates subscriptions and goes through the chosen node |
| `www.gstatic.com/generate_204` | Node latency tests (the core) |
| `1.1.1.1`, `dns.google` | The core's DNS, in TUN mode only |
| `downloads.claude.ai`, `claude.ai` | Only on **Download Claude Desktop / Claude Code** |

There is no telemetry. Prompts and answers go only between Claude and your provider through the local gateway; the search words of a web search also go to the search engines above. Those search words are written to `gateway.log` next to the settings, in plain text.

## Where things are

| What | Path |
|---|---|
| Settings and log | `%APPDATA%\app.provider-gateway-for-claude` |
| Provider keys | Windows Credential Manager, service `claude-provider-gateway` |
| Claude Desktop profile used by the app | `%LOCALAPPDATA%\ClaudeProviderGateway\Claude-3p` |
| Claude copy (~700 MB, with or without the translation) | `%LOCALAPPDATA%\ClaudeProviderGateway\claude-ru` |
| Russian translation of claude.ai | `%APPDATA%\app.provider-gateway-for-claude\claude-ru.json` |
| What the gateway learned about picky models | `%APPDATA%\app.provider-gateway-for-claude\model-profiles.json` |
| The mihomo core, its config and log | `%LOCALAPPDATA%\ClaudeProviderGateway\mihomo` |
| VPN subscription addresses | Windows Credential Manager, the same service, names `subscription:<id>` |
| Portable Claude Desktop | `%LOCALAPPDATA%\ClaudeProviderGateway\claude-desktop\app-<version>` |
| Portable Claude Code and its settings | `%LOCALAPPDATA%\ClaudeProviderGateway\claude-code` (`config` is its `CLAUDE_CONFIG_DIR`) |

claude.ai sometimes answers programs with a bot check. The app then loads the translation in its built-in browser (WebView2), which passes the check like any visitor; if it does not pass on its own within a few seconds (claude.ai wants a box ticked), that window appears so you can finish it. If it still fails, open `https://claude.ai/i18n/ru.json` in your browser and save it as that `claude-ru.json`: the compatibility check in Settings shows the exact path.

## Build from source

Requirements: Rust 1.90+ with the MSVC toolchain, Node.js (for the installer), Python 3 (optional, refreshes license notices).

```powershell
cargo test --workspace
.\build-app.ps1              # portable exe in dist\
.\build-app.ps1 -Installer   # plus the NSIS installer
```

The workspace has three crates:

- `crates/ctp-core` — configuration and model naming shared by the others;
- `crates/ctp-gateway` — the Anthropic Messages gateway, also usable on its own (`ctp-gateway --providers-file providers.json`, with the client token in `GATEWAY_TOKEN`);
- `crates/ctp-app` — the Windows app (Tauri 2).

Without the project's updater key (`TAURI_SIGNING_PRIVATE_KEY`), `-Installer` builds a plain installer with no update signatures. Releases are prepared with `tools/release.ps1` (signed installer, `latest.json`, checksums). `tools/cloudflare-mirror` is an optional update mirror for networks where GitHub is slow or unreachable.

## Contributing

Bug reports, provider templates and translations are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md) and the [changelog](CHANGELOG.md). Security issues go through private reporting, as described in [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option. Third-party components and their licenses are listed in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).
