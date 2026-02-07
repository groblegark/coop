# GitHub-backed chore queue.
#
# File a chore via `gh issue create`, dispatch to workers.

# File a GitHub chore and dispatch it to a worker.
#
# Examples:
#   oj run chore "Update dependencies to latest versions"
#   oj run chore "Add missing test coverage for auth module"
command "github:chore" {
  args = "<description>"
  run  = <<-SHELL
    gh issue create --label type:chore --title "${args.description}"
    oj worker start chore
  SHELL
}

queue "chores" {
  type = "external"
  list = "gh issue list --label type:chore --state open --json number,title --search '-label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "chore" {
  source      = { queue = "chores" }
  handler     = { job = "chore" }
  concurrency = 3
}

job "chore" {
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

  step "work" {
    run     = { agent = "chores" }
    on_done = { step = "submit" }
  }

  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      if test "$(git rev-list --count HEAD ^origin/${local.base})" -gt 0; then
        branch="${workspace.branch}" title="${local.title}"
        git push origin "$branch"
        gh issue close ${var.task.number}
        oj queue push merges --var branch="$branch" --var title="$title"
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
  on_dead = { action = "gate", run = "make check" }

  on_idle {
    action  = "nudge"
    message = <<-MSG
      Keep working. Complete the task, write tests, verify with:
      ```
      make check
      ```
      Then commit your changes.
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

  prime = ["gh issue view ${var.task.number}"]

  prompt = <<-PROMPT
    Complete the following task: #${var.task.number} - ${var.task.title}

    ## Steps

    1. Understand the task
    2. Find the relevant code
    3. Implement the changes
    4. Write or update tests
    5. Verify: `make check`
    6. Commit your changes

    If the task is already completed (e.g. by a prior commit), just commit a no-op.
  PROMPT
}
