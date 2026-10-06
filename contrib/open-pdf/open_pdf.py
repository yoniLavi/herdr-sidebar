#!/usr/bin/env -S uv run --no-project --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""open-pdf: show a PDF in pdf.js inside terminal-browser, next to the agent.

    open-pdf <file.pdf>      open the file (starts the server if it is not up)
    open-pdf --serve         run the server in the foreground
    open-pdf --url <file>    print the viewer URL and open nothing

WHY A SERVER AT ALL. Chromium gives every file:// URL its own opaque origin, so
a viewer loaded from disk starts fine and then cannot READ the PDF: fetch and
XHR of a file:// URL both fail (measured in terminal-browser 0.13.1, 2026-10-06).
Chromium's built-in PDF viewer is no way round it either: it lives in an
extension frame that terminal-browser's input never reaches, so it renders and
then ignores every click. Served over http://127.0.0.1, pdf.js is an ordinary
same-origin page and both problems go away.

WHAT THE SERVER WILL HAND OUT, which is the part that matters. It is NOT a
static file server over the disk; that would let any page in any local browser
ask it for any file. It serves exactly two things:

  /pdfjs/...            the pdf.js distribution, read-only
  /doc/<sig>/<path>     one PDF, and only when <sig> is the HMAC of <path> under
                        a secret that never leaves this machine's state dir

So a URL is a capability minted by this command for one file. Nothing else can
construct one, the server keeps no table of "opened" files to go stale, and a
viewer tab restored after a server restart still works. Requests whose Host is
not this server's own loopback address are refused, which is what stops a
hostile page reaching it by DNS rebinding.
"""

from __future__ import annotations

import hashlib
import hmac
import http.client
import json
import mimetypes
import os
import secrets
import shutil
import subprocess
import sys
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import quote, unquote, urlsplit

# --- tunables, one line each ---------------------------------------------------
PORT = int(os.environ.get("OPEN_PDF_PORT", "48765"))
PDFJS_DIR = Path(
    os.environ.get("OPEN_PDF_PDFJS_DIR", "~/.local/share/pdfjs/current")
).expanduser()
STATE_DIR = Path(
    os.environ.get("OPEN_PDF_STATE_DIR", "~/.local/state/open-pdf")
).expanduser()
TERMINAL_BROWSER = Path(
    os.environ.get("OPEN_PDF_BROWSER", "~/.local/bin/terminal-browser")
).expanduser()
# Where a NEW browser pane goes when the herdr tab has none: above the agent,
# matching the ctrl+alt+w binding and the sidebar's Browser launcher.
SPLIT_DIRECTION = "up"
SPLIT_SIZE = "0.6"
STARTUP_TIMEOUT_SECONDS = 5.0

HOST = "127.0.0.1"
SIG_HEX_CHARS = 32
# Python's table has been wrong about these on some platforms, and a module
# script served as text/plain is refused by the browser with no visible error.
MIME_OVERRIDES = {
    ".mjs": "text/javascript",
    ".js": "text/javascript",
    ".wasm": "application/wasm",
    ".ftl": "text/plain; charset=utf-8",
}


USAGE = """\
usage: open-pdf <file.pdf>      open the file (starts the server if it is not up)
       open-pdf --serve         run the server in the foreground
       open-pdf --url <file>    print the viewer URL and open nothing
"""


class OpenPdfError(Exception):
    """A failure with a message fit to show the user as-is."""


# --- the capability ------------------------------------------------------------
def load_secret(create: bool) -> bytes:
    path = STATE_DIR / "secret"
    try:
        secret = path.read_bytes().strip()
    except FileNotFoundError:
        secret = b""
    if secret:
        return secret
    if not create:
        raise OpenPdfError(f"no secret at {path}; run `open-pdf <file>` once first")
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    secret = secrets.token_hex(32).encode()
    # O_EXCL: two first runs racing must not each write a different secret and
    # leave the server holding one the client did not sign with.
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        return load_secret(create=False)
    with os.fdopen(fd, "wb") as handle:
        handle.write(secret + b"\n")
    return secret


def sign(secret: bytes, path: str) -> str:
    return hmac.new(secret, path.encode(), hashlib.sha256).hexdigest()[:SIG_HEX_CHARS]


def health_token(secret: bytes) -> str:
    """Proves the listener on PORT is OUR server, not whatever else took it."""
    return sign(secret, "open-pdf health")


def is_pdf(path: str) -> bool:
    return path.lower().endswith(".pdf")


def viewer_url(secret: bytes, pdf: Path) -> str:
    absolute = str(pdf)
    doc = f"/doc/{sign(secret, absolute)}{quote(absolute)}"
    # Quoted twice on purpose: once as a URL path, once more because the whole
    # thing travels as the value of pdf.js's ?file= parameter.
    return f"http://{HOST}:{PORT}/pdfjs/web/viewer.html?file={quote(doc, safe='')}"


# --- server --------------------------------------------------------------------
class Handler(BaseHTTPRequestHandler):
    server_version = "open-pdf"
    secret: bytes = b""

    def log_message(self, format: str, *args: object) -> None:
        sys.stderr.write(f"{self.log_date_time_string()} {format % args}\n")

    def log_request(self, code: int | str = "-", size: int | str = "-") -> None:
        # A signed URL is a capability, so it never reaches the log: the route
        # is recorded and the rest is dropped, for /doc/ and for the viewer's
        # ?file= alike.
        route = "/" + urlsplit(self.path).path.lstrip("/").split("/", 1)[0]
        self.log_message('"%s %s/…" %s', self.command, route, str(code))

    def do_GET(self) -> None:
        self.respond(send_body=True)

    def do_HEAD(self) -> None:
        self.respond(send_body=False)

    def respond(self, send_body: bool) -> None:
        if self.headers.get("Host") not in (f"{HOST}:{PORT}", f"localhost:{PORT}"):
            self.send_error(HTTPStatus.FORBIDDEN, "unexpected Host")
            return
        path = unquote(urlsplit(self.path).path)
        if path == "/healthz":
            self.send_bytes(health_token(self.secret).encode(), "text/plain", send_body)
        elif path.startswith("/pdfjs/"):
            self.send_pdfjs(path.removeprefix("/pdfjs/"), send_body)
        elif path.startswith("/doc/"):
            self.send_document(path.removeprefix("/doc/"), send_body)
        else:
            self.send_error(HTTPStatus.NOT_FOUND)

    def send_pdfjs(self, relative: str, send_body: bool) -> None:
        root = PDFJS_DIR.resolve()
        target = (root / relative).resolve()
        if not target.is_relative_to(root) or not target.is_file():
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        self.send_file(target, send_body, cache="max-age=3600")

    def send_document(self, rest: str, send_body: bool) -> None:
        sig, slash, tail = rest.partition("/")
        absolute = slash + tail
        if not hmac.compare_digest(sig, sign(self.secret, absolute)):
            self.send_error(HTTPStatus.FORBIDDEN, "bad signature")
            return
        target = Path(absolute)
        # The suffix is re-checked here, not only when signing: a signature is
        # proof the path was requested, and this is what keeps "requested"
        # meaning "a PDF" even if a caller signs something else.
        if not is_pdf(absolute) or not target.is_file():
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        self.send_file(target, send_body, cache="no-store")

    def send_file(self, target: Path, send_body: bool, cache: str) -> None:
        mime = MIME_OVERRIDES.get(target.suffix.lower())
        if mime is None:
            mime = mimetypes.guess_type(target.name)[0] or "application/octet-stream"
        try:
            handle = target.open("rb")
        except OSError:
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        with handle:
            self.send_response(HTTPStatus.OK)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(os.fstat(handle.fileno()).st_size))
            self.send_header("Cache-Control", cache)
            self.send_header("X-Content-Type-Options", "nosniff")
            self.end_headers()
            if send_body:
                shutil.copyfileobj(handle, self.wfile)

    def send_bytes(self, body: bytes, mime: str, send_body: bool) -> None:
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        if send_body:
            self.wfile.write(body)


def serve() -> None:
    if not (PDFJS_DIR / "web" / "viewer.html").is_file():
        raise OpenPdfError(f"pdf.js is not installed at {PDFJS_DIR}")
    Handler.secret = load_secret(create=True)
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    sys.stderr.write(f"open-pdf: serving on http://{HOST}:{PORT}\n")
    server.serve_forever()


# --- client --------------------------------------------------------------------
def server_state(secret: bytes) -> str:
    """'ours', 'down', or 'foreign' (something else is listening on PORT)."""
    connection = http.client.HTTPConnection(HOST, PORT, timeout=1.0)
    try:
        connection.request("GET", "/healthz")
        body = connection.getresponse().read().decode(errors="replace")
    except (ConnectionRefusedError, TimeoutError):
        return "down"
    except (OSError, http.client.HTTPException):
        return "foreign"
    finally:
        connection.close()
    return "ours" if hmac.compare_digest(body, health_token(secret)) else "foreign"


def ensure_server(secret: bytes) -> None:
    state = server_state(secret)
    if state == "ours":
        return
    if state == "foreign":
        raise OpenPdfError(
            f"port {PORT} is held by another program; set OPEN_PDF_PORT to a free one"
        )
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    log_fd = os.open(
        STATE_DIR / "server.log", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600
    )
    with os.fdopen(log_fd, "ab") as log:
        subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "--serve"],
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
    deadline = time.monotonic() + STARTUP_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if server_state(secret) == "ours":
            return
        time.sleep(0.05)
    raise OpenPdfError(
        f"the server did not come up within {STARTUP_TIMEOUT_SECONDS:g}s; "
        f"see {STATE_DIR / 'server.log'}"
    )


def resolve_pdf(argument: str) -> Path:
    pdf = Path(argument).expanduser().resolve()
    if not pdf.is_file():
        raise OpenPdfError(f"not a file: {pdf}")
    if not is_pdf(str(pdf)):
        raise OpenPdfError(f"not a .pdf: {pdf}")
    return pdf


def browser_in_this_tab() -> str | None:
    """Key of a terminal-browser already open in this herdr tab, if any."""
    result = subprocess.run(
        [str(TERMINAL_BROWSER), "ls", "--json"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return None
    try:
        browsers = json.loads(result.stdout).get("browsers", [])
    except (json.JSONDecodeError, AttributeError):
        return None
    for browser in browsers:
        if browser.get("inCurrentTab") and browser.get("key"):
            return str(browser["key"])
    return None


def show(url: str) -> None:
    if not TERMINAL_BROWSER.is_file():
        raise OpenPdfError(f"terminal-browser is not installed at {TERMINAL_BROWSER}")
    key = browser_in_this_tab()
    if key:
        argv = [str(TERMINAL_BROWSER), "new-tab", "--browser", key, url]
    else:
        argv = [
            str(TERMINAL_BROWSER),
            "open",
            url,
            "--split",
            SPLIT_DIRECTION,
            "--size",
            SPLIT_SIZE,
        ]
    result = subprocess.run(argv, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise OpenPdfError(f"terminal-browser failed: {detail}")


def main(argv: list[str]) -> int:
    try:
        if argv == ["--serve"]:
            serve()
            return 0
        if len(argv) == 2 and argv[0] == "--url":
            print(viewer_url(load_secret(create=True), resolve_pdf(argv[1])))
            return 0
        if len(argv) != 1 or argv[0].startswith("-"):
            sys.stderr.write(USAGE)
            return 2
        pdf = resolve_pdf(argv[0])
        secret = load_secret(create=True)
        ensure_server(secret)
        show(viewer_url(secret, pdf))
        return 0
    except OpenPdfError as error:
        sys.stderr.write(f"open-pdf: {error}\n")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
