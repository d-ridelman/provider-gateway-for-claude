# Security policy

## Reporting a vulnerability

Please report security issues privately through GitHub: **Security → Report a vulnerability** on this repository. Do not open a public issue for them.

Useful details: the version (Settings → Updates), what an attacker can do, and steps to reproduce. Never include real API keys or the local gateway token; replace them with placeholders.

## Supported versions

Only the latest release gets security fixes. The app checks for updates on its own unless this is turned off in Settings.

## What the app protects

- Provider API keys are stored in Windows Credential Manager, not in files.
- The gateway listens on `127.0.0.1` only; model requests without the local token are rejected.
- Errors from OpenAI-compatible providers are cleaned of anything that looks like an API key before they reach Claude or the window. Anthropic-compatible providers are passed through as they answer.
- Updates are installed only when their signature matches the public key built into the app.
- **Connect a project** runs only local programs (`claude`, `node`, `git`, `tar` from System32) with the arguments as separate values, never through a shell; it writes only into the chosen git repository, never replaces an existing file, and does nothing when Claude Code cannot list its plugins. No project file is sent anywhere.
- The built-in agent-kit is unpacked from the app itself into its own folder, through a temporary folder, and the previous copy comes back if the swap fails.
- When a critic model is chosen, the local gateway token is passed to Claude and the console opened from PGC as `AGENT_KIT_PGC_TOKEN`, so project scripts started there can call the gateway. It works only on `127.0.0.1`, but any script started from those sessions can read it.
