# Contributing

Thanks for taking the time. Bug reports, provider templates and translations are especially welcome.

## Before you start

- For anything larger than a small fix, open an issue first so we can agree on the approach.
- Never paste real API keys, tokens or full logs with private data into issues or pull requests.

## Development

Requirements: Windows 10/11 x64, Rust 1.90+ (MSVC), Node.js.

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
node tools/check_i18n.js
node tools/ui-shots.mjs      # window snapshots into state\shots\ (Chrome or Edge, no other dependencies)
node tools/ui-shots.mjs --detect   # Impeccable detector over the live pages (claude plugin enable impeccable@impeccable)
node tools/ui-shots.mjs --only docs- --out state/docs-shots   # README screenshots (neutral demo data); copy main/new/settings-en/ru to docs/images/
.\build-app.ps1              # portable exe in dist\
```

Change the window (`crates/ctp-app/ui`) only on the shared scale in `styles.css`: the `--r-*` radii and `--fs-*` type sizes, buttons 34 px (small 26 px), no new radius or font size. `node tools/ui-shots.mjs --audit` lists what is actually used.

`crates/ctp-gateway` runs on its own as well, which is handy for debugging the translation layer:

```powershell
$env:GATEWAY_TOKEN = "dev-token"   # clients send it as x-api-key
cargo run -p ctp-gateway -- --providers-file providers.json
```

## Guidelines

- Keep changes focused; a bug fix comes with a test that fails without it.
- Every user-facing string exists in both English and Russian: `tr!("ru", "en")` in Rust, `ui/i18n.js` in the interface. `node tools/check_i18n.js` checks that the two dictionaries match.
- The interface follows the system light/dark theme and must stay usable with the keyboard alone.
- Run `cargo fmt` before committing.

By contributing you agree that your work is dual-licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), like the rest of the project.
