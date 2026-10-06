# agent-open

Lets a coding agent running in a herdr pane show you a file in the sidebar's viewer. What
the sidebar does with the request is described under "Opening a file from an agent" in the
[repository README](../../README.md); this directory is the two pieces an agent needs to
reach it.

- [`herdr-open`](herdr-open) finds the installed sidebar binary and runs its `--open`.
  Needs `jq`.
- [`SKILL.md`](SKILL.md) is a [Claude Code skill](https://docs.claude.com/en/docs/claude-code/skills)
  that says when to use it and what to do when it refuses.

## Install

```bash
ln -s "$PWD/herdr-open" ~/.local/bin/herdr-open
ln -s "$PWD" ~/.claude/skills/herdr-open
```

To stop Claude Code asking before each call, allow `Bash(herdr-open:*)` in its settings.
