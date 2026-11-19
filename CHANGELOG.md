# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [0.1.0] — first public release

### Added

- Local gateway on `127.0.0.1` with the Anthropic Messages API: translates to OpenAI Chat Completions (streaming, tool calls, images, reasoning effort) or passes through to Anthropic-compatible providers.
- Windows app (Tauri 2): providers with their own key, models and reasoning ceiling; templates for OpenAI, OpenRouter, OpenCode Go and Zen, DeepSeek, Gemini, Anthropic, Ollama, llama.cpp and LM Studio.
- Model list from the provider, with names, 1M-context models and reasoning levels from the OpenCode model catalog.
- **Test connection**: one real request through the gateway.
- Claude Desktop in a separate profile; the user's own Claude account and chats are not touched.
- **Claude Code** button: a console in the project folder with `claude` already connected to the gateway.
- Compatibility self-check against the installed Claude Desktop (model rules, config fields, translation coverage).
- Optional Russian interface for Claude Desktop in a separate copy, with the official claude.ai translation.
- Interface in English and Russian, picked from the Windows display language; light and dark themes.
- Tray: closing the window keeps the gateway running.
- Updates from GitHub Releases, verified with the signing key built into the app.
- Provider keys in Windows Credential Manager; the gateway requires a local token.

[0.1.0]: https://github.com/d-ridelman/provider-gateway-for-claude/releases/tag/v0.1.0
