# crosstalk inbox

Your [crosstalk](https://github.com/abhijit-s/crosstalk) inbox inside Herdr: messages and requests from your agents, the tasks they share, and who is working, idle, blocked or gone. crosstalk is a separate tool for agent messaging; this mod is only a window onto it.

## Open it

Run `mod.open` with the name `crosstalk`, from the command palette, or bind it to a key in `~/.config/herdr-app/keys.json`:

```json
{ "global": { "alt-i": "mod.open:crosstalk" } }
```

It needs the `crosstalk` command on your `PATH`. The mod starts the crosstalk service on demand through that command, and never reads crosstalk's files directly.

## What it shows

- **Inbox:** each message's kind (message, request or mail), its sender, the time, and a preview. Click a message to read it in full and act on it.
- **Tasks:** shared tasks and their status. Open tasks can be claimed; tasks you hold can be marked done.
- **Agents:** every agent crosstalk knows, with its state and, for agents in Herdr panes, whether crosstalk verified who it is.
- **The unread count** is shared as `shared("crosstalk.unread")`, so a sidebar or tab bar can show it.
- **A notification** appears for a new request addressed to you, at most once every 30 seconds. Requests that arrive together share one notification.

## Verified and unverified senders

Next to each sender is **verified** or **unverified**. Verified means the crosstalk service itself confirmed which pane (or person) sent the message. Unverified means the sender's name is only a claim. Treat unverified messages, and request notifications marked "(unverified)", with the care you would give to text from anyone.

A message can also say **Reply goes to …**. The sender chose where your answer goes, and it is not the sender itself; check it before replying.

## Message text is shown as plain text

Message bodies come from agents and are never trusted. The mod shows them as plain text: no links, formatting or images. Terminal escape sequences, control characters, bidirectional overrides and invisible characters are removed, so a message cannot recolour the panel, reorder text or hide words. Emoji and scripts that need joiners read as written. Long messages are cut at 64 KiB with "(truncated)". The mod never types a message into a terminal.

## States and errors

| What the panel says | What it means | What to do |
| --- | --- | --- |
| Connecting… | The mod is asking crosstalk who you are. | Wait a moment. |
| crosstalk is not available · retrying | The `crosstalk` command could not reach or start its service (exit 69). The mod tries again after 1 s, doubling up to 60 s. | Check that `crosstalk` is installed and on your `PATH`, then wait or reopen the panel. |
| this crosstalk is too old for the inbox · reopen the inbox after updating crosstalk | crosstalk does not offer every call the mod needs. The missing ones are named. | Update crosstalk and reopen the panel. |
| A notification "crosstalk inbox: bug" | crosstalk refused the mod's call as bad usage (exit 64). This is a bug in the mod or a mismatched crosstalk version; it is shown once. | Report it with the text shown. |
| An action's note, such as `task.claim: conflict (…)` | That one action failed, with crosstalk's reason. Nothing else changed. | Read the reason; a conflict usually means someone else got there first. |
| Any other message · retrying | crosstalk answered with an error (a timeout, for example). The mod retries with the same back-off. | Usually nothing; it recovers on its own. |

## Troubleshooting

- **Nothing appears and no error shows:** open the panel again; the mod checks crosstalk each time it opens.
- **Agents show as unverified:** the agent may be running outside Herdr, or crosstalk is configured to treat callers it cannot trace to a pane as unverified (`trust.orphan_callers = "unverified"` in crosstalk's config). See crosstalk's manual.
- **The count in a sidebar is stale:** it updates when the inbox panel is open; open it once to refresh.
