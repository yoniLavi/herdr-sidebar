# open-pdf

Shows a PDF in [pdf.js](https://mozilla.github.io/pdf.js/) inside
[terminal-browser](https://github.com/zenbu-labs/terminal-browser), in a pane above your
agent. It is the command behind the sidebar's `openers.json` example, and works on its
own from any shell: `open-pdf report.pdf`.

Why it needs a small local server, what that server will and will not hand out, and the
tunables are at the top of [`open_pdf.py`](open_pdf.py).

## Install

Needs [uv](https://docs.astral.sh/uv/) (it runs the script) and terminal-browser.

1. Unpack a pdf.js release (`pdfjs-<version>-dist.zip` from
   <https://github.com/mozilla/pdf.js/releases>) so that
   `~/.local/share/pdfjs/current/web/viewer.html` exists. A versioned directory with a
   `current` symlink beside it makes an upgrade one `ln -sfn`.
2. `install -m 755 open_pdf.py ~/.local/bin/open-pdf`
3. Add it to `openers.json` in `herdr plugin config-dir herdr-sidebar`, then run the
   sidebar's `redeploy` action:

   ```json
   [
     { "title": "PDF viewer", "extensions": ["pdf"], "command": ["~/.local/bin/open-pdf"] }
   ]
   ```

The server starts on first use and keeps running; `~/.local/state/open-pdf/` holds its
log and the secret its URLs are signed with.
