# GitHub-backed epic queue with planning and implementation.
#
# Workflow: create epic issue → plan worker explores and writes plan →
# build worker implements the plan → PR with auto-merge → unblock cron.
#
# Blocking: single `blocked` label. Dependencies tracked in issue body
# as "Blocked by: #2, #5, #14". PRs use "Closes #N" so GitHub auto-closes
# the issue on merge. A cron runs `unblock` every 60s to detect when
# deps close and unblocks dependents.

# Create a new epic with 'plan:needed' and 'build:needed'.
#
# Examples:
#   oj run epic "Implement user authentication with OAuth"
#   oj run epic "Implement OAuth" "Support Google and GitHub providers"
#   oj run epic "Refactor storage layer" --after 5
#   oj run epic "Wire everything together" --after "3 5 14"
command "github:epic" {
  args = "<title> [body] [--after <numbers>]"
  run  = <<-SHELL
    labels="type:epic,plan:needed,build:needed"
    body="${args.body}"
    if [ -n "${args.after}" ]; then
      labels="$labels,blocked"
      refs=""
      for n in ${args.after}; do refs="$refs #$n"; done
      if [ -n "$body" ]; then
        body="$body\n\nBlocked by:$refs"
      else
        body="Blocked by:$refs"
      fi
    fi
    if [ -n "$body" ]; then
      gh issue create --label "$labels" --title "${args.title}" --body "$body"
    else
      gh issue create --label "$labels" --title "${args.title}"
    fi
    oj worker start github:plan
    oj worker start github:build
  SHELL

  defaults = {
    body  = ""
    after = ""
  }
}

# Queue existing issues for planning.
#
# Examples:
#   oj run plan 42
#   oj run plan 42 43
command "github:plan" {
  args = "<issues>"
  run  = <<-SHELL
    for num in ${args.issues}; do
      gh issue edit "$num" --add-label plan:needed
      gh issue reopen "$num" 2>/dev/null || true
    done
    oj worker start github:plan
  SHELL
}

# Queue existing issues for building (requires plan:ready).
#
# Examples:
#   oj run build 42
#   oj run build 42 43
command "github:build" {
  args = "<issues>"
  run  = <<-SHELL
    for num in ${args.issues}; do
      if ! gh issue view "$num" --json labels -q '.labels[].name' | grep -q '^plan:ready$'; then
        echo "error: #$num is missing 'plan:ready' label" >&2
        exit 1
      fi
    done
    for num in ${args.issues}; do
      gh issue edit "$num" --add-label build:needed
      gh issue reopen "$num" 2>/dev/null || true
    done
    oj worker start github:build
  SHELL
}

# Check all blocked issues and remove label when all deps are resolved.
#
# Called automatically after an epic closes. Can also be run manually.
#
# Examples:
#   oj run github:unblock
command "github:unblock" {
  run = <<-SHELL
    gh issue list --label blocked --state open --json number,body | jq -c '.[]' | while read -r obj; do
      num=$(echo "$obj" | jq -r .number)
      deps=$(echo "$obj" | jq -r '.body' | grep -i 'Blocked by:' | grep -oE '#[0-9]+' | grep -oE '[0-9]+')
      if [ -z "$deps" ]; then
        gh issue edit "$num" --remove-label blocked
        echo "Unblocked #$num (no deps)"
        continue
      fi
      all_closed=true
      for dep in $deps; do
        state=$(gh issue view "$dep" --json state -q .state 2>/dev/null)
        if [ "$state" != "CLOSED" ]; then
          all_closed=false
          break
        fi
      done
      if [ "$all_closed" = true ]; then
        gh issue edit "$num" --remove-label blocked
        echo "Unblocked #$num"
      fi
    done
  SHELL
}

# ------------------------------------------------------------------------------
# Plan queue and worker
# ------------------------------------------------------------------------------

queue "github:plans" {
  type = "external"
  list = "gh issue list --label type:epic,plan:needed --state open --json number,title --search '-label:blocked -label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:plan" {
  source      = { queue = "github:plans" }
  handler     = { job = "github:plan" }
  concurrency = 5
}

job "github:plan" {
  name      = "Plan: ${var.epic.title}"
  vars      = ["epic"]
  on_fail   = { step = "reopen" }
  on_cancel = { step = "cancel" }

  workspace {
    git    = "worktree"
    branch = "plan/${var.epic.number}-${workspace.nonce}"
  }

  locals {
    base = "main"
  }

  step "sync" {
    run     = "git fetch origin ${local.base} && git rebase origin/${local.base} || true"
    on_done = { step = "think" }
  }

  step "think" {
    run     = { agent = "plan" }
    on_done = { step = "planned" }
  }

  step "planned" {
    run = <<-SHELL
      gh issue edit ${var.epic.number} --remove-label plan:needed,in-progress --add-label plan:ready
      gh issue reopen ${var.epic.number} 2>/dev/null || true
      oj worker start github:build
    SHELL
  }

  step "reopen" {
    run = <<-SHELL
      gh issue edit ${var.epic.number} --remove-label plan:needed,in-progress --add-label plan:failed
      gh issue reopen ${var.epic.number} 2>/dev/null || true
    SHELL
  }

  step "cancel" {
    run = "gh issue close ${var.epic.number}"
  }
}

# ------------------------------------------------------------------------------
# Epic (build) queue and worker
# ------------------------------------------------------------------------------

queue "github:epics" {
  type = "external"
  list = "gh issue list --label type:epic,plan:ready,build:needed --state open --json number,title --search '-label:blocked -label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:build" {
  source      = { queue = "github:epics" }
  handler     = { job = "github:build" }
  concurrency = 5
}

job "github:build" {
  name      = "${var.epic.title}"
  vars      = ["epic"]
  on_fail   = { step = "reopen" }
  on_cancel = { step = "cancel" }

  workspace {
    git    = "worktree"
    branch = "epic/${var.epic.number}-${workspace.nonce}"
  }

  locals {
    base  = "main"
    title = "$(printf 'feat: %.76s' \"${var.epic.title}\")"
  }

  step "sync" {
    run     = "git fetch origin ${local.base} && git rebase origin/${local.base} || true"
    on_done = { step = "implement" }
  }

  step "implement" {
    run     = { agent = "implement" }
    on_done = { step = "submit" }
  }

  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      if test "$(git rev-list --count HEAD ^origin/${local.base})" -gt 0; then
        branch="${workspace.branch}"
        git push origin "$branch"
        gh pr create --title "${local.title}" --body "Closes #${var.epic.number}" --head "$branch" --label merge:auto
        gh issue edit ${var.epic.number} --remove-label build:needed,in-progress --add-label build:ready
        oj worker start github:merge
      else
        echo "No changes" >&2
        exit 1
      fi
    SHELL
  }

  step "reopen" {
    run = <<-SHELL
      gh issue edit ${var.epic.number} --remove-label build:needed,in-progress --add-label build:failed
      gh issue reopen ${var.epic.number} 2>/dev/null || true
    SHELL
  }

  step "cancel" {
    run = "gh issue close ${var.epic.number}"
  }
}

# ------------------------------------------------------------------------------
# Merge queue and worker (fast-path: clean rebases only)
# ------------------------------------------------------------------------------

queue "github:merges" {
  type = "external"
  list = "gh pr list --label merge:auto --json number,title,headRefName"
  take = "gh pr edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:merge" {
  source      = { queue = "github:merges" }
  handler     = { job = "github:merge" }
  concurrency = 1
}

# Fast-path: clean rebases only. Failures are forwarded to the CI/CD resolve queue.
job "github:merge" {
  name      = "Merge PR #${var.pr.number}: ${var.pr.title}"
  vars      = ["pr"]
  on_cancel = { step = "cleanup" }

  workspace = "folder"

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "merge-pr-${var.pr.number}-${workspace.nonce}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      git -C "${local.repo}" fetch origin main
      git -C "${local.repo}" fetch origin pull/${var.pr.number}/head:${local.branch}
      git -C "${local.repo}" worktree add "${workspace.root}" ${local.branch}
    SHELL
    on_done = { step = "rebase" }
  }

  step "rebase" {
    run     = "git rebase origin/main"
    on_done = { step = "verify" }
    on_fail = { step = "queue-cicd" }
  }

  step "verify" {
    run     = "make check"
    on_done = { step = "push" }
    on_fail = { step = "queue-cicd" }
  }

  step "queue-cicd" {
    run = <<-SHELL
      git rebase --abort 2>/dev/null || true
      gh pr edit ${var.pr.number} --remove-label merge:auto --add-label merge:cicd
      oj worker start github:cicd
    SHELL
    on_done = { step = "cleanup" }
  }

  step "push" {
    run = <<-SHELL
      git push --force-with-lease origin HEAD:${var.pr.headRefName}
      gh pr merge ${var.pr.number} --squash --auto
      gh pr edit ${var.pr.number} --remove-label in-progress
      issue=$(gh pr view ${var.pr.number} --json body -q '.body' | grep -oE 'Closes #[0-9]+' | grep -oE '[0-9]+' | head -1)
      if [ -n "$issue" ]; then
        gh issue edit "$issue" --remove-label build:ready
      fi
    SHELL
    on_done = { step = "cleanup" }
  }

  step "cleanup" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
    SHELL
  }
}

# ------------------------------------------------------------------------------
# CI/CD resolve queue and worker (slow-path: agent-assisted resolution)
# ------------------------------------------------------------------------------

queue "github:cicd" {
  type = "external"
  list = "gh pr list --label merge:cicd --json number,title,headRefName"
  take = "gh pr edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:cicd" {
  source      = { queue = "github:cicd" }
  handler     = { job = "github:cicd" }
  concurrency = 1
}

# Slow-path: agent-assisted conflict resolution and build fixes.
job "github:cicd" {
  name      = "Resolve PR #${var.pr.number}: ${var.pr.title}"
  vars      = ["pr"]
  on_cancel = { step = "cleanup" }

  workspace = "folder"

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "merge-pr-${var.pr.number}-${workspace.nonce}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      git -C "${local.repo}" fetch origin main
      git -C "${local.repo}" fetch origin pull/${var.pr.number}/head:${local.branch}
      git -C "${local.repo}" worktree add "${workspace.root}" ${local.branch}
    SHELL
    on_done = { step = "rebase" }
  }

  step "rebase" {
    run     = "git rebase origin/main"
    on_done = { step = "verify" }
    on_fail = { step = "resolve" }
  }

  step "verify" {
    run     = "make check"
    on_done = { step = "push" }
    on_fail = { step = "resolve" }
  }

  step "resolve" {
    run     = { agent = "merge-resolver" }
    on_done = { step = "push" }
  }

  step "push" {
    run = <<-SHELL
      git push --force-with-lease origin HEAD:${var.pr.headRefName}
      gh pr merge ${var.pr.number} --squash --auto
      gh pr edit ${var.pr.number} --remove-label merge:cicd,in-progress
      issue=$(gh pr view ${var.pr.number} --json body -q '.body' | grep -oE 'Closes #[0-9]+' | grep -oE '[0-9]+' | head -1)
      if [ -n "$issue" ]; then
        gh issue edit "$issue" --remove-label build:ready
      fi
    SHELL
    on_done = { step = "cleanup" }
  }

  step "cleanup" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
    SHELL
  }
}

# ------------------------------------------------------------------------------
# Unblock cron
# ------------------------------------------------------------------------------

cron "github:unblock" {
  interval = "60s"
  run      = { job = "github:unblock" }
}

job "github:unblock" {
  name = "unblock"

  step "check" {
    run = <<-SHELL
      gh issue list --label blocked --state open --json number,body | jq -c '.[]' | while read -r obj; do
        num=$(echo "$obj" | jq -r .number)
        deps=$(echo "$obj" | jq -r '.body' | grep -i 'Blocked by:' | grep -oE '#[0-9]+' | grep -oE '[0-9]+')
        if [ -z "$deps" ]; then
          gh issue edit "$num" --remove-label blocked
          echo "Unblocked #$num (no deps)"
          continue
        fi
        all_closed=true
        for dep in $deps; do
          state=$(gh issue view "$dep" --json state -q .state 2>/dev/null)
          if [ "$state" != "CLOSED" ]; then
            all_closed=false
            break
          fi
        done
        if [ "$all_closed" = true ]; then
          gh issue edit "$num" --remove-label blocked
          echo "Unblocked #$num"
        fi
      done
    SHELL
  }
}

# ------------------------------------------------------------------------------
# Agents
# ------------------------------------------------------------------------------

agent "plan" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools EnterPlanMode,ExitPlanMode"
  on_dead = { action = "resume", attempts = 1 }

  session "tmux" {
    color = "blue"
    title = "Plan: #${var.epic.number}"
    status { left = "#${var.epic.number}: ${var.epic.title}" }
  }

  prime = <<-SHELL
    cat <<'GATE'
    ## Acceptance Gate

    Your work is only accepted if you post a plan comment on the issue.
    If you crash or exit without posting, the job will be retried.
    GATE

    echo ''
    echo '## Issue'
    gh issue view ${var.epic.number}
  SHELL

  prompt = <<-PROMPT
    Create an implementation plan for GitHub issue #${var.epic.number}: ${var.epic.title}

    1. Spawn 3-5 Explore agents in parallel (depending on complexity)
    2. Spawn a Plan agent to synthesize findings
    3. Add the plan as a comment: `gh issue comment ${var.epic.number} -b "the plan"`
  PROMPT
}

agent "implement" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools EnterPlanMode,ExitPlanMode"
  on_dead = { action = "resume", attempts = 1 }

  on_idle {
    action  = "nudge"
    message = <<-MSG
      Keep working. Implement, verify with `make check`, then commit.
    MSG
  }

  session "tmux" {
    color = "blue"
    title = "Epic: #${var.epic.number}"
    status {
      left  = "#${var.epic.number}: ${var.epic.title}"
      right = "${workspace.branch}"
    }
  }

  prime = <<-SHELL
    cat <<'GATE'
    ## Acceptance Gate

    Your work is REJECTED if `make check` does not pass.
    You MUST run `make check` and fix any failures before committing.
    If you exit without a passing `make check`, the job will be retried.
    GATE

    echo ''
    echo '## Issue & Plan'
    gh issue view ${var.epic.number} --comments
  SHELL

  prompt = <<-PROMPT
    Implement GitHub issue #${var.epic.number}: ${var.epic.title}

    The plan is in the issue comments above.

    1. Follow the plan
    2. Implement
    3. Verify: `make check` (MUST pass — this is your acceptance gate)
    4. Commit
  PROMPT
}

agent "merge-resolver" {
  run     = "claude --model sonnet --dangerously-skip-permissions"
  on_idle = { action = "gate", command = "test ! -d $(git rev-parse --git-dir)/rebase-merge" }
  on_dead = { action = "escalate" }

  session "tmux" {
    color = "yellow"
    title = "Merge: PR #${var.pr.number}"
    status {
      left  = "${var.pr.title}"
      right = "${var.pr.headRefName} -> main"
    }
  }

  prime = <<-SHELL
    echo '## Git Status'
    git status

    echo ''
    echo '## PR'
    gh pr view ${var.pr.number}

    echo ''
    echo '## Commits (branch vs main)'
    git log --oneline origin/main..HEAD 2>/dev/null || git log --oneline REBASE_HEAD~1..REBASE_HEAD 2>/dev/null || true

    echo ''
    echo '## Recent build output'
    make check 2>&1 | tail -80 || true
  SHELL

  prompt = <<-PROMPT
    You are landing PR #${var.pr.number} ("${var.pr.title}") onto main.

    Something went wrong — either a rebase conflict or a build failure.
    Diagnose from the git status and build output above, fix it, and
    get `make check` passing.

    If mid-rebase: resolve conflicts, `git add`, `git rebase --continue`, repeat.
    If build fails: fix the code, amend the commit.

    Done when: rebase is complete and `make check` passes.
  PROMPT
}
