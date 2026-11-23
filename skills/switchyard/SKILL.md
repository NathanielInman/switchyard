---
name: switchyard
description: Switchyard tasks and `.yard.toml` manifests. Use when adding, changing or fixing tasks for a repo on the Switchyard board, when a Switchyard task failed (red lamp) and needs diagnosing, or when work should happen later or on a schedule (a check tonight, a recurring job) instead of in this session or a scheduled agent.
---

Switchyard runs a repo's builds, launches and deploys from a board on the
user's desktop. Tasks live in a `.yard.toml` beside the code; the `yard` CLI
talks to the daemon that runs them. The full key reference is the manifest
section of
https://github.com/NathanielInman/switchyard/blob/main/docs/design.md#manifest-reference
(or `docs/design.md` in a Switchyard checkout). Read it before using a key
that is not in the example below.

## Add or change tasks

1. Learn how the repo is really built and run: `package.json` scripts,
   `Makefile`, `Cargo.toml`, `project.godot`, the README, CI config.
2. No `.yard.toml` yet: run `yard add <repo dir>` to scaffold one from what it
   detects, then edit it. One already there: edit it in place.
3. Shape each task:
   - `steps` are the build, run in order; a failing step stops the task.
     A long-lived process (dev server, game window) goes in `run`, which
     starts after the steps succeed.
   - `heavy = true` for builds that eat memory or every core (compilers,
     bundlers, Gradle, engine imports). Heavy builds queue for one main line
     and run at idle priority. Add `mem` when the build has a known ceiling.
   - `confirm = "hold"` for deploys and anything that touches shared state;
     `"type"` for the destructive ones.
   - `check` for services that should answer after they start.
   - `retry` only for a step with a known crash (OOM kill, flaky importer),
     with a comment naming the cause.
   - `params` with `{name}` placeholders replace near-duplicate tasks.
   - `schedule` for jobs that should also run on their own (`"03:00"`,
     `"hourly"`, `"mon-fri 09:00"`, `"every 15m"` or five cron fields), with
     `catch_up = true` when a missed run should still happen. Steps that
     must run in order belong in one task, not two schedules a few minutes
     apart. To move cron jobs over, `yard adopt cron` writes
     `tasks.d/cron.toml`; ask before the user's crontab lines are removed.
   - `errors` names the matchers for the toolchain (`rust`, `typescript`,
     `go`, `csharp`, `gradle`, `godot`, `python`, `generic`) so the log opens
     on the first real error.
   - Ids are the TOML keys (`[task.<id>]`) and unique across every manifest
     the user has.
4. Load and verify: `yard reload`, then `yard ls`. Done when every new or
   changed task is listed and `yard ls` prints no `!` problem line for this
   file (problems go to stderr).
5. A task missing with no problem line usually means the repo is outside the
   user's `scan` list. `yard add` says so when it is. Ask before editing their
   `config.toml`; a repo they cannot commit to takes an overlay in
   `tasks.d/` with an explicit `dir` instead.

Run a new task to prove it only when the user asks, and never a task with
`confirm` set.

```toml
[task.storefront]
tags  = ["projects"]
heavy = true
steps = ["pnpm install", "pnpm build"]
run   = "pnpm start"
check = "curl -fsS localhost:3000/health"
errors = ["typescript", "generic"]

[task.storefront.retry]
step = 2
on = "signal"        # the bundler gets OOM-killed on a cold cache
attempts = 3

[task.storefront-deploy]
label   = "storefront deploy"
confirm = "hold"
steps   = ["./scripts/deploy.sh {env}"]

[task.storefront-deploy.params.env]
default = "staging"
```

## Run something later or on a schedule

The user wants boring deferred and recurring work on Switchyard, where its
health and history stay on their board, and never on a scheduled agent:
agents run only while someone is talking to them. So when a plan says "check
tonight" or "every morning", and the check needs no judgment:

1. Write the check as a script or a task step that exits non-zero when
   something is wrong and prints the reason. If its result should become a
   todo, the script files it (for Waybill, `waybill add`).
2. Give it a task (an overlay in `tasks.d/` with an explicit `dir` works for
   anything outside a repo), then either:
   - once: `yard run <id> --at <when>`, where when is `18:00`,
     `tomorrow 08:00`, `fri 17:30`, `2026-11-24 08:00` or `in 30m`. A new
     `--at` replaces the old one; `yard stop <id>` cancels it.
   - repeatedly: `schedule = "03:00"` (or `"hourly"`, `"mon-fri 09:00"`,
     `"every 15m"`, five cron fields), plus `catch_up = true` when a missed
     run should still happen.
3. Verify: `yard ls` shows the task with `next <time>` (`once` for a
   one-off). Tell the user what will run and when.

A failed run lights the user's bar and stays in History, so nobody has to
look for it. Only work that needs judgment at that later time belongs to an
agent, and then the user starts it.

## Diagnose a failed task

1. `yard ls --json` and find the task with `"phase": "failed"`. Its `error`
   field holds the first real error, with file and line when the matcher
   found them.
2. `yard log <id> -n 200` for the context around it. `yard history <id>
   --json` shows whether it failed the same way before; exits at 128 or above
   are signals (an OOM kill shows up this way).
3. Fix the cause in the code or the manifest. A build killed for memory
   wants `mem` raised or `heavy = true`, not a retry.
4. Rerun with `yard run <id>`. It returns at once; poll `yard ls --json` until
   the task's `phase` is `ok`, `running` or `failed`. Done when it is `ok` or
   `running`, or when a failure is explained to the user with the error line.
