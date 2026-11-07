---
name: fds-cli
description: >-
  Operate XiaoMi FDS (File Data Storage) object storage from the command line
  using this repo's `fds_cli` tool — upload/download/list/copy/rename/delete
  objects, manage buckets, read/write object metadata and ACLs, generate
  presigned & absolute URLs, do multipart uploads, run concurrent/resumable
  downloads, and run an end-to-end `smoke` verification. Use this skill whenever
  the user wants to put/get/list/delete/copy/move objects on FDS, manage FDS
  buckets, share a file via a presigned/temporary/download link, check whether an
  object or bucket exists, or verify FDS connectivity — even if they don't say
  "fds_cli" by name, as long as the target is XiaoMi FDS (endpoints ending in
  .fds.api.xiaomi.com or -fds.api.xiaomi.net). Prefer this over hand-writing
  curl/HTTP requests to FDS, because fds_cli signs requests (Galaxy-V2) correctly.
---

# fds_cli — operate XiaoMi FDS object storage

`fds_cli` is the CLI that ships with this repo's `fds` SDK. It talks to XiaoMi
FDS (File Data Storage, an S3-like object store) and handles the Galaxy-V2
request signing for you, so use it instead of hand-rolling signed HTTP requests.

## 1. Prepare (do this once per session)

**Build the binary** (from the repo root; `cli` is an opt-in feature so the
library itself never pulls `clap`):

```bash
cargo build --features cli --bin fds_cli
FDS=./target/debug/fds_cli        # use this path for every command below
```

**Credentials** come from environment variables (or per-command flags). Check
whether they're already set; if not, **ask the user for the endpoint + keys —
never invent or guess credentials**:

```bash
export GO_FDS_TEST_ENDPOINT="cnbj1.fds.api.xiaomi.com"   # must end in .fds.api.xiaomi.com or -fds.api.xiaomi.net
export GO_FDS_TEST_ACCESS_KEY_ID="<AK>"
export GO_FDS_TEST_ACCESS_KEY_SECRET="<SK>"
```

The equivalent flags are `--endpoint / --access-key-id / --access-key-secret`
(global, placed before the subcommand). `--help` on any subcommand shows its
exact args: `$FDS object put --help`.

## 2. Workflow

1. Identify the operation the user wants and map it to the right subcommand
   (§4). If unsure which object/bucket they mean, ask rather than guess.
2. Run the command. Most write commands print a small confirmation or a response
   struct; read commands print data.
3. **Verify** the result with a follow-up read (§5) and report concretely (e.g.
   "uploaded, and `object exists` confirms it's there"), instead of assuming
   success.

## 3. Safety — confirm before destructive actions ⚠️

These commands **permanently remove data**. Before running any of them, show the
user exactly what will be affected and get explicit confirmation:

- `object delete`, `object delete-many` — echo the object name(s) first.
- `object delete-prefix` — this can wipe many objects. **First run
  `object list <bucket> --prefix <p> --all` and show the count/names**, then ask
  "delete these N objects under `<p>`?" before deleting.
- `bucket delete` — a bucket must be empty; confirm the user really means it.

Never chain a delete off an assumption. When in doubt, list first, confirm, then
delete. Non-destructive reads (list/get/exists/info/basic-info/get-metadata/
get-acl/url/presign) need no confirmation.

## 4. Command reference

`$FDS` = `./target/debug/fds_cli`. Bucket `<B>`, object key `<O>` (may contain
`/`, which is part of the key). Full per-command examples live in
`docs/cli.md` in this repo.

### Objects
```bash
$FDS object put <B> <O> --text "hi"                 # or --file <path>
$FDS object put <B> <O> --file ./a.bin --content-type application/octet-stream \
     --meta x-xiaomi-meta-author=alice               # custom meta keys must start with x-xiaomi-meta-
$FDS object get <B> <O>                              # body to stdout
$FDS object get <B> <O> --out ./a.bin                # body to a file
$FDS object get <B> <O> --range "bytes=0-1023"       # partial
$FDS object exists <B> <O>                           # prints true / false
$FDS object list <B> --prefix logs/ --delimiter / --max-keys 100 [--all]
$FDS object copy <SRCB> <SRCO> <DSTB> <DSTO>
$FDS object rename <B> <SRC> <DST>
$FDS object restore <B> <O>                          # un-archive
$FDS object basic-info <B> <O>                       # size, storage class, metadata
$FDS object get-metadata <B> <O>
$FDS object set-metadata <B> <O> --meta x-xiaomi-meta-k=v [--meta ...]
$FDS object get-acl <B> <O>
$FDS object set-public <B> <O>                       # make world-readable
# destructive (confirm first, see §3):
$FDS object delete <B> <O>
$FDS object delete-many <B> a.txt b.txt [--trash]
$FDS object delete-prefix <B> <prefix> [--trash]
```

### Buckets
```bash
$FDS bucket exists <B>                               # true / false
$FDS bucket info <B>
$FDS bucket list                                     # all buckets
$FDS bucket list-authorized
$FDS bucket create <B> [--storage-class STANDARD] [--org-id ...]
$FDS bucket get-acl <B>
$FDS bucket get-lifecycle <B> [--rule-id r1]
$FDS bucket set-lifecycle-rule <B> --file rule.json
$FDS bucket set-lifecycle <B> --file config.json
$FDS bucket get-accesslog <B>
$FDS bucket set-accesslog <B> --enabled --log-bucket <LB> [--log-prefix access/]
$FDS bucket migrate <B> --org-id <o> --team-id <t>
$FDS bucket delete <B>                               # destructive, confirm (bucket must be empty)
```

### Sharing links (local, no network — safe)
```bash
$FDS presign <B> <O> --method GET --expires 3600 [--cdn]   # temporary signed URL
$FDS presign <B> <O> --method PUT --expires 600            # hand to an uploader
$FDS url <B> <O>                                           # absolute URL (unsigned; only public objects)
```

### Large files
```bash
$FDS multipart-upload <B> <O> --file ./big.zip --part-size 5242880   # parts >= 5 MiB except last
$FDS download <B> <O> ./out.bin --concurrency 8 --part-size 1048576 [--breakpoint]
```

### End-to-end verification
```bash
$FDS smoke <B>                     # runs put/get/metadata/acl/copy/rename/list/multipart/download/cleanup, prints ✓/✗
$FDS smoke <B> --skip-multipart    # faster (skips the ~6 MiB multipart step)
```

## 5. Verify after acting

Prefer a concrete check over an assumption:

- after **put** → `object get <B> <O>` (compare bytes) or `object exists` → `true`.
- after **copy/rename** → `object exists` on the new key → `true` (and, for
  rename, the old key → `false`).
- after **delete** → `object exists` → `false`.
- after **set-metadata / set-public** → re-read with `get-metadata` / `get-acl`.
- to check a **presigned GET** really works: `curl -s "<url>"` returns the
  object (200); a 403 means a signing/clock problem, not a link-format problem.
- whole-system check: `$FDS smoke <B>` → all `✓` and exit code 0.

## 6. Troubleshooting

- `missing --access-key-id ...` / `... --endpoint` — credentials/endpoint not set
  (env vars or flags). Ask the user; don't guess.
- `wrong endpoint` — endpoint must end in `.fds.api.xiaomi.com` or
  `-fds.api.xiaomi.net`.
- `Code: [403] ...` — signature/permission/clock. Isolate with
  `curl "$($FDS presign <B> <O>)"`: 200 ⇒ signing fine (so it's a permission
  issue on the failing call); 403 ⇒ credentials or system clock.
- `Code: [404] ...` — object/bucket doesn't exist. Confirm the key with `list`.
- multipart "part too small" — raise `--part-size` to ≥ `5242880` (5 MiB); only
  the last part may be smaller.

For the exhaustive command + verification manual, read `docs/cli.md`; for SDK
usage, `docs/usage.md`.
