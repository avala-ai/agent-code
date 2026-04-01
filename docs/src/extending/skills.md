
Skills are reusable prompt templates that define multi-step workflows. They're markdown files with YAML frontmatter, loaded from `.agent/skills/` or `~/.config/agent-code/skills/`.

## Creating a skill

Create a file in `.agent/skills/test-and-fix.md`:

```markdown

Run the test suite with the project's test command. If any tests fail:

1. Read the failing test file
2. Read the source code being tested
3. Identify the root cause
4. Fix the issue
5. Re-run tests to verify

Repeat until all tests pass. Do not skip or delete failing tests.
```

## Frontmatter options

| Field | Type | Description |
|-------|------|-------------|
| `description` | string | What this skill does |
| `whenToUse` | string | Hints for the LLM about when to suggest this skill |
| `userInvocable` | boolean | Whether users can invoke it via `/skill-name` |
| `disableNonInteractive` | boolean | Disable in one-shot mode |
| `paths` | string[] | File patterns that trigger this skill suggestion |

## Invoking skills

### As a slash command

If `userInvocable: true`, invoke with the filename (minus `.md`):

```
> /test-and-fix
```

### With arguments

Use `{{arg}}` in the template for argument substitution:

```markdown

Review {{arg}} for bugs, security issues, and code quality problems.
Focus on edge cases and error handling.
```

```
> /review src/auth.rs
```

### Programmatically via the Skill tool

The LLM can invoke skills when it determines one is appropriate:

```json
{
  "name": "Skill",
  "input": {
    "skill": "test-and-fix",
    "args": null
  }
}
```

## Directory skills

For complex skills with supporting files, use a directory:

```
.agent/skills/
  deploy/
    SKILL.md      ← the skill definition
    checklist.md  ← referenced by the skill
```

## Skill locations

| Location | Scope |
|----------|-------|
| `.agent/skills/` | Project-specific |
| `~/.config/agent-code/skills/` | Available in all projects |

## Commands

```
> /skills
Loaded 3 skills:
  test-and-fix [invocable] — Run tests and fix failures
  review [invocable] — Review a specific file
  deploy — Production deployment checklist
```
