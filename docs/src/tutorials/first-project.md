
This tutorial walks through using agent-code on a real project for the first time.

## Prerequisites

- agent-code installed (`agent --version` works)
- An API key configured (any provider)
- A project directory with code in it

## Step 1: Navigate to your project

```bash
cd /path/to/your/project
```

agent-code uses your current directory as context. It can read files, run commands, and make edits here.

## Step 2: Start the agent

```bash
agent
```

You'll see the welcome banner with your session ID. The agent is ready.

## Step 3: Explore the codebase

Ask the agent to understand your project:

```
> what is this project and how is it structured?
```

The agent will use `Glob` to find files, `FileRead` to read key files (README, package.json, Cargo.toml, etc.), and explain the structure.

## Step 4: Make a change

Try something concrete:

```
> add a health check endpoint that returns {"status": "ok"}
```

The agent will:
1. Read existing code to understand patterns
2. Find where endpoints are defined
3. Write the new endpoint
4. Run tests if they exist

Watch the tool calls — you'll see `FileRead`, `Grep`, `FileWrite`, and `Bash` in action.

## Step 5: Review what changed

```
> /diff
```

This shows the git diff of everything the agent modified.

## Step 6: Commit if you're happy

```
> /commit
```

The agent reviews the diff and creates a commit with a descriptive message.

## Step 7: Save project context

Create an `AGENTS.md` file so the agent remembers your project in future sessions:

```
> /init
```

Or ask the agent to create one:

```
> create an AGENTS.md with our project's tech stack, conventions, and test commands
```

## What's next

- Use `/plan` to explore code safely (read-only mode)
- Use `/review` to review your changes before committing
- Use `/model` to switch to a faster or more capable model
- See [Custom Skills](custom-skills) to create reusable workflows
