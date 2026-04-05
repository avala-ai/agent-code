
Skills turn multi-step workflows into single commands. This tutorial creates a skill from scratch.

## What we'll build

A `/deploy-check` skill that verifies a project is ready for deployment: tests pass, no uncommitted changes, and the build succeeds.

## Step 1: Create the skill file

```bash
mkdir -p .agent/skills
```

Create `.agent/skills/deploy-check.md`:

```markdown
---
description: Verify the project is ready for deployment
userInvocable: true
---

Run a pre-deployment checklist:

1. Check for uncommitted changes with `git status`. If there are
   uncommitted changes, warn the user and stop.

2. Run the project's test suite. If any tests fail, report the
   failures and stop.

3. Run the build command. If it fails, report the error and stop.

4. If everything passes, report "Ready to deploy" with a summary
   of what was checked.

Do not proceed past a failing step.
```

## Step 2: Verify it loaded

Start agent-code and check:

```
> /skills
```

You should see `deploy-check [invocable]` in the list.

## Step 3: Run it

```
> /deploy-check
```

The agent follows the steps in order, stopping at the first failure.

## Adding arguments

Skills support `{{arg}}` substitution. Create `.agent/skills/review-file.md`:

```markdown
---
description: Deep review of a specific file
userInvocable: true
---

Review {{arg}} thoroughly:

1. Read the file and understand its purpose
2. Check for bugs, edge cases, and error handling gaps
3. Check for security issues (injection, XSS, auth bypass)
4. Suggest specific improvements with line references
```

Use it:

```
> /review-file src/auth.rs
```

## Directory skills

For complex skills with supporting context, use a directory:

```
.agent/skills/
  deploy-check/
    SKILL.md          ← the skill definition
    checklist.md      ← referenced by the skill
    known-issues.md   ← context the agent can read
```

The skill file must be named `SKILL.md` in a directory skill.

## Sharing skills

Skills are just markdown files. Share them by:

- Committing `.agent/skills/` to your repo (team-wide)
- Copying to `~/.config/agent-code/skills/` (personal, all projects)
- Publishing as a plugin (see [Plugins](../extending/plugins))

## Tips

- Keep skill prompts specific — vague instructions produce vague results
- Number the steps — the agent follows numbered lists reliably
- Include stop conditions ("if X fails, stop and report")
- Test with `/skills` to verify loading before running
