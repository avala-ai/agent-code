
Every interactive session is automatically saved when you exit. You can resume any previous session to continue where you left off.

## Auto-save

Sessions save automatically on exit (Ctrl+D or `/exit`). The session file includes:

- Full conversation history (messages, tool calls, results)
- Turn count and model used
- Working directory at session start
- Session ID

Sessions are stored as JSON in `~/.config/agent-code/sessions/`.

## Resume a session

List recent sessions:

```
> /sessions
Recent sessions:

  a1b2c3d — /home/user/project (5 turns, 23 msgs, 2026-03-31T20:15:00Z)
  e4f5g6h — /home/user/other (12 turns, 67 msgs, 2026-03-31T18:30:00Z)

Use /resume <id> to restore a session.
```

Resume by ID:

```
> /resume a1b2c3d
Resumed session a1b2c3d (23 messages, 5 turns)
```

The agent now has the full conversation context from the previous session and can continue the work.

## Session ID

Each session gets a short unique ID shown in the welcome banner:

```
 rc  session a1b2c3d
```

Use this ID to resume later.

## Export

Export the current conversation as markdown:

```
> /export
Exported to conversation-export-20260331-201500.md
```

The export includes user messages and assistant responses as readable markdown.
