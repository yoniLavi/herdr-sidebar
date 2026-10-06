---
name: herdr-open
description: Show a file to the user in the herdr sidebar's preview pane, above the agent's own pane. Use when the user asks to open, show, preview or look at a file, or when you have just written a document they asked for and will read. Only inside herdr (HERDR_PANE_ID is set).
---

# herdr-open

Run this to put a file in front of the user:

```bash
herdr-open <path>
herdr-open <path>:<line>
```

The file appears in the preview pane above your pane, in your own tab. Focus does not
move, so the user keeps typing wherever they were.

## When to use it

- The user asks to open, show, preview or look at a file.
- You have finished a document the user asked for and they will want to read it.

Do not open files as a running commentary on your own work. One file the user wants is
useful; every file you touch is noise, and each call replaces what they were reading.

Skip it when `HERDR_PANE_ID` is unset: you are not in herdr and the command will refuse.

## When it refuses

It exits 0 and prints one line when the file is showing. Otherwise it exits 1, prints
the reason on stderr, and has changed nothing. The refusals are deliberate:

- The viewer has a file open for editing. The user is working in it.
- Your tab has no viewer yet and is not the tab the user is looking at. Starting one
  would pull their focus to you.
- The path is not a file.

On any refusal, tell the user what it said and give them the path. Do not retry, and do
not work around it by closing panes, sending keys, or opening the file some other way.
