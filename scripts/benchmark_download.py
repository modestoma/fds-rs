#!/usr/bin/env python3
"""macOS client-only RSS benchmark; server and verification stay in Python.
Run: cargo build --offline -p fds --example download_memory
     python3 scripts/benchmark_download.py target/debug/examples/download_memory
"""
import hashlib
import http.server
import pathlib
import re
import subprocess
import sys
import tempfile
import threading
import time

SIZE = 64 * 1024 * 1024
CHUNK = b'x' * 65536

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.send_response(200)
        self.send_header('Content-Length', str(SIZE))
        self.send_header('ETag', '"benchmark-v1"')
        self.end_headers()

    def do_GET(self):
        match = re.fullmatch(r'bytes=(\d+)-(\d+)', self.headers['Range'])
        start, end = map(int, match.groups())
        self.send_response(206)
        self.send_header('Content-Length', str(end-start+1))
        self.send_header('Content-Range', f'bytes {start}-{end}/{SIZE}')
        self.send_header('ETag', '"benchmark-v1"')
        self.end_headers()
        left = end-start+1
        while left:
            data = CHUNK[:min(left, len(CHUNK))]
            self.wfile.write(data)
            left -= len(data)

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
expected = hashlib.md5()
for _ in range(SIZE // len(CHUNK)):
    expected.update(CHUNK)
print('concurrency,part_MiB,client_peak_RSS_MiB,seconds,verified_bytes')
try:
    with tempfile.TemporaryDirectory(prefix='fds-memory-') as directory:
        for concurrency, part_mib in [(1, 1), (4, 1), (4, 8), (8, 8)]:
            path = pathlib.Path(directory) / 'out'
            start = time.monotonic()
            run = subprocess.run(['/usr/bin/time', '-l', sys.argv[1],
                                  f'http://127.0.0.1:{server.server_port}', str(path),
                                  str(part_mib * 1024 * 1024), str(concurrency)],
                                 capture_output=True, text=True)
            if run.returncode:
                raise RuntimeError(run.stderr)
            elapsed = time.monotonic() - start
            rss = int(re.search(r'(\d+)\s+maximum resident set size', run.stderr)[1])
            digest = hashlib.md5()
            with path.open('rb') as file:
                while data := file.read(65536):
                    digest.update(data)
            assert path.stat().st_size == SIZE and digest.digest() == expected.digest()
            print(f'{concurrency},{part_mib},{rss/1024/1024:.2f},{elapsed:.3f},{SIZE}')
finally:
    server.shutdown()
    server.server_close()
