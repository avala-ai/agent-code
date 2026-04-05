
## File Operations

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `FileRead` | Read files with line numbers. Handles text, PDF, notebooks, images. | Yes | Yes |
| `FileWrite` | Create or overwrite files. Auto-creates parent dirs. | No | No |
| `FileEdit` | Search-and-replace. Requires unique match or `replace_all`. | No | No |
| `MultiEdit` | Batch multiple edits in a single atomic operation. | No | No |
| `NotebookEdit` | Edit Jupyter cells (replace, insert, delete). | No | No |

## Search

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `Grep` | Regex search via ripgrep. Context lines, glob filter, case control. | Yes | Yes |
| `Glob` | Find files by pattern, sorted by modification time. | Yes | Yes |
| `ToolSearch` | Discover tools by keyword or `select:Name`. | Yes | Yes |

## Execution

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `Bash` | Shell commands. Background mode, destructive detection, sandbox, timeout. | No | No |
| `PowerShell` | PowerShell commands (Windows). | No | No |
| `REPL` | Python or Node.js code execution. | No | No |

## Agent Coordination

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `Agent` | Spawn subagents. Optional worktree isolation. | No | No |
| `SendMessage` | Inter-agent communication. | No | Yes |
| `Skill` | Invoke skills programmatically. | Yes | No |

## Planning and Tracking

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `EnterPlanMode` | Switch to read-only mode. | Yes | Yes |
| `ExitPlanMode` | Re-enable all tools. | Yes | Yes |
| `TaskCreate` | Create a progress tracking task. | Yes | Yes |
| `TaskUpdate` | Update task status. | Yes | Yes |
| `TaskGet` | Get task details by ID. | Yes | Yes |
| `TaskList` | List all session tasks. | Yes | Yes |
| `TaskStop` | Stop a running background task. | No | Yes |
| `TaskOutput` | Read output of a completed task. | Yes | Yes |
| `TodoWrite` | Structured todo list management. | Yes | Yes |

## Web and External

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `WebFetch` | HTTP GET with HTML-to-text conversion. | Yes | Yes |
| `WebSearch` | Web search with result extraction. | Yes | Yes |
| `LSP` | Language server diagnostics with linter fallbacks. | Yes | Yes |

## MCP Integration

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `McpProxy` | Call tools on connected MCP servers. | No | No |
| `ListMcpResources` | Browse MCP server resources. | Yes | Yes |
| `ReadMcpResource` | Read an MCP resource by URI. | Yes | Yes |

## Workspace

| Tool | Description | Read-only | Concurrent |
|------|-------------|:---------:|:----------:|
| `EnterWorktree` | Create an isolated git worktree. | No | No |
| `ExitWorktree` | Clean up a worktree. | No | No |
| `AskUserQuestion` | Interactive multi-choice prompts. | Yes | No |
| `Sleep` | Async pause (max 5 min). | Yes | Yes |
