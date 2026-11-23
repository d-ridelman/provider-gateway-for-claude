"""Writes THIRD-PARTY-NOTICES.txt: every crate linked into the Windows app with its
license expression and the license/notice texts shipped in the crate.

Usage (from the repository root): python tools/third_party_notices.py [out-file]
Needs `cargo` on PATH; reads only local registry sources (no network).
"""
import json
import subprocess
import sys
from pathlib import Path

TARGET = "x86_64-pc-windows-msvc"
OWN = {"ctp-core", "ctp-gateway", "ctp-app"}
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "copyright", "unlicense")


def metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", TARGET, "--locked"],
        check=True, capture_output=True, text=True, encoding="utf-8",
    ).stdout
    return json.loads(out)


def linked_packages(meta: dict) -> list[dict]:
    """Packages reachable from the app through normal (non-dev, non-build) dependencies."""
    nodes = {node["id"]: node for node in meta["resolve"]["nodes"]}
    packages = {pkg["id"]: pkg for pkg in meta["packages"]}
    root = next(pid for pid, pkg in packages.items() if pkg["name"] == "ctp-app")
    seen, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return sorted(
        (packages[pid] for pid in seen if packages[pid]["name"] not in OWN),
        key=lambda pkg: (pkg["name"], pkg["version"]),
    )


def license_texts(pkg: dict) -> list[tuple[str, str]]:
    folder = Path(pkg["manifest_path"]).parent
    files = []
    if pkg.get("license_file"):
        files.append(folder / pkg["license_file"])
    files += sorted(
        p for p in folder.iterdir()
        if p.is_file() and p.name.lower().startswith(LICENSE_PREFIXES) and p not in files
    )
    texts = []
    for path in files:
        try:
            texts.append((path.name, path.read_text(encoding="utf-8", errors="replace").strip()))
        except OSError:
            pass
    return texts


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "THIRD-PARTY-NOTICES.txt")
    packages = linked_packages(metadata())
    lines = [
        "Third-party software notices",
        "============================",
        "",
        "Provider Gateway for Claude includes the open-source components listed below.",
        "Each is used under its own license, reproduced after the list.",
        "",
        "Programs it can download on request are NOT included and are not part of it: they",
        "come from their authors' own release servers and run as separate processes.",
        "- mihomo (Clash.Meta core) — GPL-3.0 — https://github.com/MetaCubeX/mihomo",
        "  (the pinned version and its sha256 are in crates/ctp-app/src/mihomo.rs; its source",
        "  code is at the tag of that version in the repository above)",
        "- Claude Desktop and Claude Code — Anthropic's own terms — https://claude.ai",
        "",
    ]
    # Identical texts (most MIT and Apache-2.0 copies) are printed once, with their crates.
    by_text: dict[str, list[str]] = {}
    missing = []
    for pkg in packages:
        label = f"{pkg['name']} {pkg['version']}"
        source = f" — source: {pkg['repository']}" if pkg.get("repository") else ""
        lines.append(f"- {label} — {pkg.get('license') or 'see license text'}{source}")
        texts = license_texts(pkg)
        if not texts:
            missing.append(pkg["name"])
        for _, text in texts:
            by_text.setdefault(text, []).append(label)
    if missing:
        lines += ["", "Crates without a license file in their package (licensed by the SPDX expression",
                  "above; full texts of those licenses appear below): " + ", ".join(missing)]
    for number, (text, crates) in enumerate(by_text.items(), start=1):
        lines += ["", "-" * 78, f"License text {number}, used by: {', '.join(crates)}", "", text]
    out.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{out}: {len(packages)} crates, without license files: {', '.join(missing) or 'none'}")


if __name__ == "__main__":
    main()
