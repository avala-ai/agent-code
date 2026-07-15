
The sandbox restricts what the agent process and its child commands can access on disk and (where supported) the network, using OS primitives:

| Platform | Backend |
|----------|---------|
| Linux | `bwrap` (bubblewrap) when available |
| macOS | Seatbelt (`sandbox-exec`) |
| Other / missing binary | No-op strategy (logged) |

Sandboxing is an **extra** layer on top of the permission system. It does not replace ask/allow rules or protected-directory blocks.

## Enable / disable

```bash
# Disable for one session (ignored if enterprise lock is on)
agent --no-sandbox

# Config — see [security] / sandbox settings in configuration docs
```

If `security.disable_bypass_permissions = true`, operators cannot disable the sandbox via CLI flags.

## How it interacts with permissions

1. **Hooks** can still deny a tool before it runs  
2. **Permission rules** (deny / ask / allow) still apply  
3. **Sandbox** constrains the OS view of the process that runs the tool  

A tool may be “allowed” by the permission modal and still fail if the sandbox profile blocks the path.

## Recommendations

| Scenario | Suggestion |
|----------|------------|
| Local trusted repo | Default permissions; sandbox optional |
| Untrusted code review | Strict permissions + sandbox when available |
| CI with fixed tree | `--permission-mode allow` + known-good sandbox install |
| Enterprise fleet | `disable_bypass_permissions = true` |

## Troubleshooting

- **Linux “noop” strategy** — install `bubblewrap` (`bwrap` on `PATH`)
- **macOS denials** — check Seatbelt profile logs; some tools need broader read paths
- **Unexpected write failures** — confirm path is not in protected dirs (`.git/`, `node_modules/`, …)

## Related

- [Permissions](./concepts/permissions.md)
- [Security](./security.md)
- [Configuration](./configuration/settings.md)
