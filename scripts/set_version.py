"""Set the release version everywhere it is written down.

Usage: python3 scripts/set_version.py X.Y.Z

semantic-release calls this in its prepare step. It rewrites the workspace
version (inherited by both crates), the fds-rs workspace dependency, and the
dependency / install versions documented in README.md and docs/. Run
`cargo update --workspace` afterwards to refresh Cargo.lock.
"""
from pathlib import Path
import re
import sys

if len(sys.argv) != 2 or not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", sys.argv[1]):
    raise SystemExit("usage: set_version.py X.Y.Z")
version = sys.argv[1]
root = Path(__file__).resolve().parents[1]

def rewrite(path, pattern, count_expected=None):
    text = path.read_text()
    new, count = re.subn(pattern, lambda m: m.group(1) + version + m.group(2), text, flags=re.M)
    if count_expected is not None and count != count_expected:
        raise SystemExit(f"{path.relative_to(root)}: expected {count_expected} match(es) of {pattern!r}, found {count}")
    path.write_text(new)

manifest = root / "Cargo.toml"
# [workspace.package] version, then the fds-rs workspace dependency.
rewrite(manifest, r'^(version = ")[^"]+(")', 1)
rewrite(manifest, r'^(fds = \{ package = "fds-rs", path = "crates/fds", version = ")[^"]+(")', 1)

for path in [root / "README.md", *sorted((root / "docs").rglob("*.md"))]:
    rewrite(path, r'((?:fds-rs|fds_cli)\s*=\s*(?:\{\s*version\s*=\s*)?")[^"]+(")')
    rewrite(path, r"(--version\s+)\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(\b)")

print(f"version set to {version}")
