# GitHub-backed bug queue.
#
# File a bug via `gh issue create`, dispatch to fix workers.

# File a GitHub bug and dispatch it to a fix worker.
#
# Examples:
#   oj run fix "Button doesn't respond to clicks"
#   oj run fix "Login page crashes on empty password"
command "github:fix" {
  args = "<description>"
  run  = <<-SHELL
    gh issue create --label type:bug --title "${args.description}"
    oj worker start github:bug
  SHELL
}

queue "github:bugs" {
  type = "external"
  list = "gh issue list --label type:bug --state open --json number,title --search '-label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:bug" {
  source      = { queue = "github:bugs" }
  handler     = { job = "github:bug" }
  concurrency = 3
}

job "github:bug" {
  name      = "${var.bug.title}"
  vars      = ["bug"]
  on_fail   = { step = "reopen" }
  on_cancel = { step = "cancel" }

  workspace {
    git    = "worktree"
    branch = "fix/${var.bug.number}-${workspace.nonce}"
  }

  locals {
    base  = "main"
    title = "$(printf 'fix: %.75s' \"${var.bug.title}\")"
  }

  notify {
    on_start = "Fixing: ${var.bug.title}"
    on_done  = "Fix landed: ${var.bug.title}"
    on_fail  = "Fix failed: ${var.bug.title}"
  }

  step "fix" {
    run     = { agent = "bugs" }
    on_done = { step = "submit" }
  }

  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      if test "$(git rev-list --count HEAD ^origin/${local.base})" -gt 0; then
        branch="${workspace.branch}"
        git push origin "$branch"
        gh pr create --title "${local.title}" --body "Closes #${var.bug.number}" --head "$branch" --label merge:auto
        oj worker start github:merge
      elif gh issue view ${var.bug.number} --json state -q '.state' | grep -q 'CLOSED'; then
        echo "Issue already resolved, no changes needed"
      else
        echo "No changes to submit" >&2
        exit 1
      fi
    SHELL
  }

  step "reopen" {
    run = <<-SHELL
      gh issue edit ${var.bug.number} --remove-label in-progress
      gh issue reopen ${var.bug.number} 2>/dev/null || true
    SHELL
  }

  step "cancel" {
    run = "gh issue close ${var.bug.number}"
  }
}

agent "bugs" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools ExitPlanMode,EnterPlanMode"
  on_dead = { action = "resume", attempts = 1 }

  on_idle {
    action  = "nudge"
    message = <<-MSG
      Keep working. Fix the bug, verify with `make check`, then commit.
    MSG
  }

  session "tmux" {
    color = "blue"
    title = "Bug: #${var.bug.number}"
    status {
      left  = "#${var.bug.number}: ${var.bug.title}"
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
    echo '## Issue'
    gh issue view ${var.bug.number}
  SHELL

  prompt = <<-PROMPT
    Fix GitHub issue #${var.bug.number}: ${var.bug.title}

    1. Understand the bug
    2. Find the relevant code
    3. Implement a fix
    4. Write or update tests
    5. Verify: `make check` (MUST pass — this is your acceptance gate)
    6. Commit your changes

    If the bug is already fixed (e.g. by a prior commit), just commit a no-op.
  PROMPT
}
