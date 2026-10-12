---
name: commit-push
description: Commit the working tree and push to the current branch on origin. Use when the user asks to commit, push, save work to git, or runs /commit-push.
---

# Commit and push

Commits what is in the working tree and pushes it to **the current branch's
HEAD on `origin`**.

## Hard constraints

- The repository already exists and its remote is already correct:
  `origin  git@github.com:nvcble/StageLink.git`. **Never** run `git init`,
  `git remote add`, `git remote set-url`, or `gh repo create`. If the remote
  does not match, stop and report it — do not fix it.
- Push to the branch that is already checked out. This repository's default
  branch is `main` and that is where work lands, so **do not create a branch**
  unless the user asks.
- Never `--force`, never `--amend` an already-pushed commit, never rewrite
  history.
- Never stage a file the user did not change as part of this work. Check the
  status before staging; `git add -A` is fine when the whole tree is this
  change and wrong when it is not.

## Steps

1. Look before you commit, in one call:

   ```bash
   git status --short && git remote -v && git log --oneline -5 && git diff --stat HEAD
   ```

   Confirm the remote is `nvcble/StageLink`. If anything in the status is
   unrelated to the work — a stray artefact, a local experiment, a key or an
   `.env` — say so and ask, rather than committing it.

2. If `versions.json` or anything derived from it is in the diff, confirm they
   agree:

   ```bash
   node scripts/version.mjs --check
   ```

3. Stage and commit. Write the message to a file and use `-F` so the body
   keeps its line breaks:

   ```bash
   git add -A && git commit -F .git/COMMIT_MSG_STAGELINK
   ```

4. Push:

   ```bash
   git push origin HEAD
   ```

   If the push is rejected as non-fast-forward, **stop**. Report it and let the
   user decide whether to rebase — do not resolve it unilaterally.

5. Report the commit subject, the short SHA, the branch, and the file count.

## Message style

Match `git log`. This repository does **not** use Conventional Commits.

- **Subject**: one line, lower case after any leading label, no trailing full
  stop. An em dash to join a change to its consequence is the house style —
  `Step 7: encryption — a release build can finally stream`.
- **Body**: prose in markdown, wrapped at 72–76 columns. Explain *why*, and
  what it means for a performer or an operator, not a list of the diff. Use
  `##` headings only when the change really has separate parts.
- **Say what was verified, in bold**, and how. `**Verified on this machine:**`
  followed by the actual evidence.
- **Say what is still unverified, plainly.** A limitation stated in the commit
  is worth more than one discovered at a service. This is the project's
  standing rule, not a preference.
- End with test counts when they moved, then the attribution line:

  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  ```

Claim nothing the tests did not show. If a build or a suite was not run, the
message must not imply it was.
