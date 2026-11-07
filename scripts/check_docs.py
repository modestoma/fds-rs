"""Check local Markdown link targets and reject retired SDK API references."""
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit

root = Path(__file__).resolve().parents[1]
documents = [root / "README.md", *sorted((root / "docs").rglob("*.md"))]
errors = []
checked = 0

def anchors(path):
    content = re.sub(r"^```[^\n]*\n.*?^```[^\n]*$", "", path.read_text(), flags=re.M | re.S)
    result = set(re.findall(r'<a\s+(?:id|name)="([^"]+)"', content))
    counts = {}
    for heading in re.findall(r"^#{1,6}\s+(.+?)\s*#*\s*$", content, re.M):
        heading = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", heading)
        slug = re.sub(r"[^\w\- ]", "", heading.lower()).replace(" ", "-")
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        result.add(f"{slug}-{count}" if count else slug)
    return result

for path in documents:
    # Code examples may contain Markdown-like syntax that is not a link.
    content = re.sub(r"^```[^\n]*\n.*?^```[^\n]*$", "", path.read_text(), flags=re.M | re.S)
    links = re.findall(r"\]\(([^\s)]+)(?:\s+\"[^\"]*\")?\)", content)
    links += re.findall(r"^\s*\[[^\]]+\]:\s*(\S+)", content, re.M)
    for link in links:
        url = urlsplit(link.strip("<>"))
        if url.scheme or url.netloc:
            continue
        target = (path.parent / unquote(url.path)).resolve() if url.path else path
        checked += 1
        if not target.exists():
            errors.append(f"{path.relative_to(root)}: missing local target {link}")
        elif url.fragment and target.suffix == ".md" and unquote(url.fragment) not in anchors(target):
            errors.append(f"{path.relative_to(root)}: missing heading {link}")

# Removed APIs, files and conventions must not reappear in docs or code.
retired = ["create_bucket_with_acl", "set_bucket_acl_with_action", "UploadPartList",
           "raw_metadata", "raw_iter", "lookup(", "set_limiter", "cslb-prometheus",
           "async-channel", "migration.md", "ManagerError", "DeleteObjectsError",
           "MultipartUploadError", "DownloadRequest", "GetObjectRequest",
           "PutObjectRequest", "ListObjectsRequest", "UploadPartRequest", "into_raw",
           "_with_response", "send(&client)", "fds::download", "GO_FDS_TEST"]
files = [*documents, root / "Cargo.toml", root / "Cargo.lock", *sorted((root / "crates").rglob("Cargo.toml"))]
for directory in ("crates",):
    files.extend((root / directory).rglob("*.rs"))
for path in files:
    content = path.read_text()
    for token in retired:
        if token in content:
            errors.append(f"{path.relative_to(root)}: retired API/feature {token}")
# Every example is listed in the README, which links to its source.
readme = (root / "README.md").read_text()
examples = sorted((root / "crates/fds/examples").glob("*.rs"))
for example in examples:
    if f"](crates/fds/examples/{example.name})" not in readme:
        errors.append(f"README.md: example {example.name} is not linked")

# Documented dependency versions follow the workspace version, which both
# crates inherit; scripts/set_version.py rewrites all of them on release.
import tomllib
workspace = tomllib.loads((root / "Cargo.toml").read_text())
version = workspace["workspace"]["package"]["version"]
if workspace["workspace"]["dependencies"]["fds"]["version"] != version:
    errors.append("Cargo.toml: workspace dependency fds-rs version differs from the workspace version")
for name in ("fds", "fds_cli"):
    package = tomllib.loads((root / "crates" / name / "Cargo.toml").read_text())["package"]
    if package.get("version") != {"workspace": True}:
        errors.append(f"crates/{name}/Cargo.toml: version must be inherited from the workspace")
for path in documents:
    content = path.read_text()
    for found in re.findall(r'(?:fds-rs|fds_cli)\s*=\s*(?:\{\s*version\s*=\s*)?"([^"]+)"', content):
        if found != version:
            errors.append(f"{path.relative_to(root)}: dependency version {found} != {version}")
    for found in re.findall(r"--version\s+(\d+\.\d+\.\d+)", content):
        if found != version:
            errors.append(f"{path.relative_to(root)}: install version {found} != {version}")

if errors:
    raise SystemExit("\n".join(errors))
print(f"{checked} local Markdown targets exist; {len(examples)} examples linked; "
      f"documented version {version}; retired API/feature scan passed")
