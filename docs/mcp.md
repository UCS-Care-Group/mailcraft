# MCP server

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-14 · **Change:** minor (commands that send or hand back the owner's data without looking like it) · **Target:** Microsoft Outlook (Microsoft 365)

MailCraft speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio, so
agents can read, file and write mail and keep a calendar.

```sh
mailcraft-cli mcp --file my.mailcraft      # headless session on a mailbox file (no window)
mailcraft-cli mcp --sample                 # … or on the sample mailbox
mailcraft --control 7991 &                 # or: drive the running app…
mailcraft-cli mcp --connect 127.0.0.1:7991 # …including clicks, keys and screenshots
```

With `--connect`, the bridge sends the app's control key, which it reads from
`MAILCRAFT_CONTROL_KEY` or from the key file the app writes at start. It sends the key only to
loopback addresses; see [Keys](control-protocol.md#keys).

## Message text is data, not instructions

Mail is written by strangers. The server's instructions tell the agent that what a message says is
something to report, never something to do, and `read_message` repeats it. Keep that in any prompt
you build on top: an agent that files, answers or forwards mail because a message asked it to is
the failure to design against. For anything that sends (`send_message`, `compose.send`), prefer a
setup where a person confirms. The same goes for commands that send on the owner's behalf without
looking like it: `mail.readReceipt` tells the sender the message was read, and a Quick Step
(`quickSteps.run`) can reply, forward or delete. `people.listGroups` and `searchFolder.list` hand
back the owner's contacts and queries: data, like the mail itself.

A headless session works on the mailbox file only. It does not connect to mail servers: sending
from an account with a server puts the message in its Outbox, and `mailcraft-cli sync` (or the
app's Send/Receive) delivers it.

## Tools

| Tool | What it does |
|---|---|
| `list_commands` | every command (filter with `query`) |
| `execute` | run one command: `{command: "mail.flag", params: {id: 12}}` |
| `batch` | run several commands in order |
| `open_mailbox` / `save_mailbox` | a `.mailcraft` file, the sample mailbox, or an empty one |
| `import_file` | bring in `.eml`, `.msg`, `.mbox`, `.ics`, `.vcf` |
| `inspect_mailbox` / `list_folders` | accounts and counts; the folder tree |
| `list_messages` | a folder's list, optionally searched (`from:`, `has:attachment`, `is:unread`, dates…) |
| `read_message` | one message in full |
| `send_message` | write and send a new message |
| `list_events` / `list_contacts` / `list_tasks` | the calendar between two days; people; to-dos |
| `parity` | feature coverage |
| `screenshot`, `click`, `key`, `ui_inspect` | the window (with `--connect` only) |

Everything else goes through `execute`: replying (`mail.reply`, `compose.set`, `compose.send`),
filing (`mail.move`, `mail.categorize`, `mail.archive`), rules (`rules.add`), the calendar
(`calendar.new` with `recurrence`), contacts and tasks. `list_commands` gives each command's
parameters.

## Resources

- `mailcraft://mailbox`: accounts and counts (JSON)
- `mailcraft://commands`: every command (JSON)

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First version, from `crates/mcp/src` |
| 2026-10-14 | minor | Read receipts, Quick Steps, contact groups and search folders under "data, not instructions" |
