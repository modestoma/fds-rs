"""Check the source API index, SDK mappings and saved YAML fingerprints."""
from collections import Counter
from pathlib import Path
import hashlib
import re

root = Path(__file__).resolve().parents[1]
source_dir = root / "docs/open_api"
files = {path.name: path for path in source_dir.glob("*.yaml")}
error_files = {"ClientError.yaml", "ServerError.yaml"}
operations = set(files) - error_files
index = (source_dir / "README.md").read_text()
errors = []

# Index rows must link each source exactly once. Hash rows use plain code names.
rows = re.findall(r"^\| \[[^\]]+\]\(([^)]+\.yaml)\) \| (.+)$", index, re.M)
counts = Counter(name for name, _ in rows)
if len(files) != 35 or len(operations) != 33:
    errors.append("source inventory changed: review the expected 35 files / 33 operations")
if set(counts) != set(files):
    errors.append(f"index mismatch: missing={sorted(set(files) - set(counts))}, extra={sorted(set(counts) - set(files))}")
if any(count != 1 for count in counts.values()):
    errors.append("each YAML file must have exactly one index row")

mapped = {}
for name, row in rows:
    cells = [cell.strip() for cell in row.rstrip("|").split("|")]
    if name in operations:
        if len(cells) != 4 or cells[2] not in {"implemented", "duplicate", "missing"} or not cells[3]:
            errors.append(f"{name}: expected function, wire, SDK status and mapping")
        else:
            mapped[name] = cells[2]
if set(mapped) != operations:
    errors.append("all operation definitions must retain a valid SDK mapping")

fingerprints = re.findall(r"^\| `([^`]+\.yaml)` \| `([0-9a-f]{64})` \|$", index, re.M)
if len(fingerprints) != len(files) or {name for name, _ in fingerprints} != set(files):
    errors.append("fingerprints must cover every source file exactly once")
for name, expected in fingerprints:
    if name in files and hashlib.sha256(files[name].read_bytes()).hexdigest() != expected:
        errors.append(f"{name}: SHA-256 mismatch; review source changes before updating the index")

if errors:
    raise SystemExit("\n".join(errors))
print("35/35 YAML files indexed and fingerprinted; 33/33 operations mapped; this is not live proof")
