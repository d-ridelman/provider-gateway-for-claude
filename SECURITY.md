# Security policy

## Reporting a vulnerability

Please report security issues privately through GitHub: **Security → Report a vulnerability** on this repository. Do not open a public issue for them.

Useful details: the version (Settings → Updates), what an attacker can do, and steps to reproduce. Never include real API keys or the local gateway token; replace them with placeholders.

## Supported versions

Only the latest release gets security fixes. The app checks for updates on its own unless this is turned off in Settings.

## What the app protects

- Provider API keys are stored in Windows Credential Manager, not in files.
- The gateway listens on `127.0.0.1` only and rejects requests without the local token.
- Provider error messages are cleaned of anything that looks like an API key before they reach Claude or the window.
- Updates are installed only when their signature matches the public key built into the app.
