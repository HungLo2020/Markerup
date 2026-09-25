---
name: update-claude-skills
description: Link skills from .agents/skills into .claude/skills so Claude Code can load them. Use when the user asks to update, sync, or link Claude skills, or after a skill is added to .agents/skills.
---

# Update Claude skills

Skills live in `.agents/skills/<name>/`. Claude Code only discovers skills in `.claude/skills/`, so each skill needs a relative symlink there:

```
.claude/skills/<name> -> ../../.agents/skills/<name>
```

Never copy or move skill files into `.claude`. The skill's real files stay in `.agents`.

From the repository root:

1. List the skills: every directory in `.agents/skills/` that contains a `SKILL.md`. Report any directory without a `SKILL.md` and skip it.
2. Create `.claude/skills/` if it does not exist.
3. For each skill, check `.claude/skills/<name>`:
   - **Missing:** create the link with `ln -s ../../.agents/skills/<name> .claude/skills/<name>`.
   - **Symlink to `../../.agents/skills/<name>`:** already correct; leave it.
   - **Anything else** (a real directory or file, or a symlink to somewhere else): do not replace or delete it. Report it and let the user decide.
4. Check for symlinks in `.claude/skills/` that point into `.agents/skills/` but whose target no longer exists. Report them; remove them only if the user asks.
5. Verify every new link resolves, for example `test -f .claude/skills/<name>/SKILL.md`.
6. Report what you linked, what was already linked, and anything you skipped or found broken. Do not make or push commits unless the user explicitly asks.

The steps above as one script, which creates only missing links and prints everything else:

```bash
mkdir -p .claude/skills
for dir in .agents/skills/*/; do
  name=$(basename "$dir")
  link=".claude/skills/$name"
  target="../../.agents/skills/$name"
  if [ ! -f "$dir/SKILL.md" ]; then echo "skipped (no SKILL.md): $name"
  elif [ -L "$link" ] && [ "$(readlink "$link")" = "$target" ]; then echo "already linked: $name"
  elif [ -e "$link" ] || [ -L "$link" ]; then echo "conflict, left unchanged: $link"
  else ln -s "$target" "$link" && echo "linked: $name"
  fi
done
for link in .claude/skills/*; do
  [ -L "$link" ] && [ ! -e "$link" ] && echo "broken link: $link -> $(readlink "$link")"
done
```

Claude Code picks up new skills in `.claude/skills` during the session, usually without a restart. If a new skill does not appear, start a new session.
