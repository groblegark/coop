# GitHub-backed chore queue.
#
# File a chore via `gh issue create`, dispatch to workers.

# File a GitHub chore and dispatch it to a worker.
#
# Examples:
#   oj run chore "Update dependencies to latest versions"
#   oj run chore "Add missing test coverage for auth module" "Details here..."
command "github:chore" {
  args = "<title> [body]"
  run  = <<-SHELL
    if [ -n "${args.body}" ]; then
      gh issue create --label type:chore --title "${args.title}" --body "${args.body}"
    else
      gh issue create --label type:chore --title "${args.title}"
    fi
    oj worker start github:chore
  SHELL

  defaults = {
    body = ""
  }
}

queue "github:chores" {
  type = "external"
  list = "gh issue list --label type:chore --state open --json number,title --search '-label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:chore" {
  source      = { queue = "github:chores" }
  handler     = { job = "github:chore" }
  concurrency = 3
}

job "github:chore" {
  name      = "${var.task.title}"
  vars      = ["task"]
  on_fail   = { step = "reopen" }
  on_cancel = { step = "cancel" }

  workspace {
    git    = "worktree"
    branch = "chore/${var.task.number}-${workspace.nonce}"
  }

  locals {
    base  = "main"
    title = "$(printf 'chore: %.73s' \"${var.task.title}\")"
  }

  notify {
    on_start = "Chore: ${var.task.title}"
    on_done  = "Chore done: ${var.task.title}"
    on_fail  = "Chore failed: ${var.task.title}"
  }

  step "sync" {
    run     = "git fetch origin ${local.base} && git rebase origin/${local.base} || true"
    on_done = { step = "work" }
  }

  step "work" {
    run     = { agent = "chores" }
    on_done = { step = "submit" }
  }

  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      if test "$(git rev-list --count HEAD ^origin/${local.base})" -gt 0; then
        branch="${workspace.branch}"
        git push origin "$branch"
        gh pr create --title "${local.title}" --body "Closes #${var.task.number}" --head "$branch" --label merge:auto
        oj worker start github:merge
      elif gh issue view ${var.task.number} --json state -q '.state' | grep -q 'CLOSED'; then
        echo "Issue already resolved, no changes needed"
      else
        echo "No changes to submit" >&2
        exit 1
      fi
    SHELL
  }

  step "reopen" {
    run = <<-SHELL
      gh issue edit ${var.task.number} --remove-label in-progress
      gh issue reopen ${var.task.number} 2>/dev/null || true
    SHELL
  }

  step "cancel" {
    run = "gh issue close ${var.task.number}"
  }
}

agent "chores" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools ExitPlanMode,EnterPlanMode"
  on_dead = { action = "resume", attempts = 1 }

  on_idle {
    action  = "nudge"
    message = <<-MSG
      Keep working. Complete the task, verify with `make check`, then commit.
    MSG
  }

  session "tmux" {
    color = "blue"
    title = "Chore: #${var.task.number}"
    status {
      left  = "#${var.task.number}: ${var.task.title}"
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
    gh issue view ${var.task.number}
  SHELL

  prompt = <<-PROMPT
    Complete GitHub issue #${var.task.number}: ${var.task.title}

    1. Understand the task
    2. Find the relevant code
    3. Implement the changes
    4. Write or update tests
    5. Verify: `make check` (MUST pass — this is your acceptance gate)
    6. Commit your changes

    If the task is already completed (e.g. by a prior commit), just commit a no-op.
  PROMPT
}
