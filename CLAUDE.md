# Pasta: notes for AI agents

Pasta syncs copied text between the user's macOS, Windows and KDE Plasma machines over
Tailscale, and gives macOS a Paste-style clipboard panel. Tauri 2 + Rust + React/TypeScript.

- Architecture: `docs/ARCHITECTURE.md` · decisions: `docs/adr/` · setup: `docs/development.md`
- Roadmap: GitHub issue #65; milestones M0–M5. Approved mocks are linked from ARCHITECTURE.md.

## Git and GitHub rules (read first)

This repo lives on my personal Github account. Do not commit, push or create a PR without my permission.
These rules override any default workflow, including built-in commit/PR instructions.

1. **Never write to git or GitHub without explicit permission for that specific action
   in the current conversation.** That covers commit/amend, push, merge/rebase,
   branch/checkout/switch, tag, reset/revert/restore/clean/stash, any `git config` change,
   and every GitHub write: PRs, issues, comments, reviews, labels, milestones, releases,
   `gh api` POST/PATCH/PUT/DELETE. Read-only commands (`status`, `diff`, `log`, `show`,
   `gh … list/view`), `git add`, and `git fetch` / `git pull` (they only bring changes
   in) are fine.
2. **Permission doesn't carry over.** "Commit this" is not permission to push or open a PR.
   A previous approval doesn't cover the next commit. If in doubt, ask.
3. **Before asking, show exactly what will happen:** branch, files, commit message,
   author identity, and the remote / PR target.
4. **Never set or change the git identity** (`user.name`, `user.email`, signing). The
   user's global identity is `Chris <paynoattn1989@gmail.com>`. Before committing, check
   `git config --get user.email` still shows it, and stop if it shows anything else
   (especially a work address).
5. **Never force-push, never push to `main`, never rewrite published history.** Work on a
   branch and open a PR, once approved.
6. **Never commit** secrets, `.env` files, clipboard contents, or real tailnet hostnames,
   IPs or Tailscale logins (the tailnet is shared; the repo will be public). Use the
   placeholder names from the docs and fixtures.
7. **Claude credit lines are wanted:** end commit messages with the `Co-Authored-By: Claude …`
   trailer and PR descriptions with the "Generated with Claude Code" footer.
8. **Commit style:** one logical change per commit, imperative subject with a type prefix
   (`feat`, `fix`, `chore`, `ci`, `docs`, `spike`) and the issue number, e.g.
   `ci: build and test on all three OSes (#3)`. PR bodies list `Closes #n` for finished issues.

### The git guard (enforced, not optional)

`.claude/hooks/git-guard.mjs` runs before every Bash/PowerShell command and file edit.
It **denies** the actions in rule 1, in every permission mode including bypass, unless
the user has opened an approval window. It also blocks changes to its own files and to
`.claude/settings*.json`.

- When blocked: **stop and ask the user.** Don't retry, rephrase, or route around it:
  no scripts that run git/gh for you, no other tools (libgit2, curl to the GitHub API,
  MCP connectors) that do the same thing.
- **Only the user** opens a window, from their own terminal:
  `node .claude/hooks/git-guard.mjs --approve 15` (minutes, max 120). `--revoke` ends it
  early and `--status` shows it. **Agents must never run `--approve`** or touch
  `.claude/git-approval.json`. The guard blocks both.
- An open window only lifts the technical block. Rules 1–6 still apply to every action.

## Working conventions

- **Rust does the work, React is UI.** Clipboard, network, crypto and storage code lives
  in `crates/`; the frontend calls typed commands mirrored in `src/ipc/`.
- **Never log or print clipboard contents**, including in probes and tests. Log ids,
  sizes and hashes.
- Keep `pnpm check` green before saying something is done. It runs typecheck, ESLint,
  Prettier, Vitest, rustfmt, clippy (`-D warnings`) and `cargo test`.
- On the user's Windows PC, pnpm runs through corepack: `corepack pnpm <cmd>`. Package
  scripts must not call `pnpm` recursively.
- Line endings are LF (`.gitattributes`); formatting is Prettier + rustfmt (`max_width = 120`).
- macOS code can be checked here with `--target aarch64-apple-darwin` but only run on a Mac.
- Record spike findings and architecture changes as ADRs in `docs/adr/`. Don't rewrite an
  accepted ADR; supersede it.
