# GitHub-backed epic queue with planning and implementation.
#
# Workflow: create epic issue → plan worker explores and writes plan →
# epic worker implements the plan → merge queue.

# Create a new epic with 'plan:needed' and 'build:needed'.
#
# Examples:
#   oj run epic "Implement user authentication with OAuth"
#   oj run epic "Refactor storage layer" --blocked 5
command "github:epic" {
  args = "<description> [--blocked <number>]"
  run  = <<-SHELL
    labels="type:epic,plan:needed,build:needed"
    if [ -n "${args.blocked}" ]; then
      labels="$labels,blocked:${args.blocked}"
    fi
    gh issue create --label "$labels" --title "${args.description}"
    oj worker start plan
    oj worker start epic
  SHELL

  defaults = {
    blocked = ""
  }
}

# Create a new epic with 'plan:needed' only (no auto-build).
#
# Examples:
#   oj run idea "Add caching layer for API responses"
command "idea" {
  args = "<description>"
  run  = <<-SHELL
    gh issue create --label type:epic,plan:needed --title "${args.description}"
    oj worker start plan
  SHELL
}

# Queue existing issues for planning.
#
# Examples:
#   oj run plan 42
#   oj run plan 42 43
command "plan" {
  args = "<issues>"
  run  = <<-SHELL
    for num in ${args.issues}; do
      gh issue edit "$num" --add-label plan:needed
      gh issue reopen "$num" 2>/dev/null || true
    done
    oj worker start plan
  SHELL
}

# Queue existing issues for building (requires plan:ready).
#
# Examples:
#   oj run build 42
#   oj run build 42 43
command "build" {
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
    oj worker start epic
  SHELL
}

# ------------------------------------------------------------------------------
# Plan queue and worker
# ------------------------------------------------------------------------------

queue "plans" {
  type = "external"
  list = "gh issue list --label type:epic,plan:needed --state open --json number,title --search '-label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "plan" {
  source      = { queue = "plans" }
  handler     = { job = "plan" }
  concurrency = 3
}

job "plan" {
  name      = "Plan: ${var.epic.title}"
  vars      = ["epic"]
  on_fail   = { step = "reopen" }
  on_cancel = { step = "cancel" }

  step "think" {
    run     = { agent = "plan" }
    on_done = { step = "planned" }
  }

  step "planned" {
    run = <<-SHELL
      gh issue edit ${var.epic.number} --remove-label plan:needed,in-progress --add-label plan:ready
      gh issue reopen ${var.epic.number} 2>/dev/null || true
      oj worker start epic
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

queue "epics" {
  type = "external"
  list = <<-SHELL
    gh issue list --label type:epic,plan:ready,build:needed --state open --json number,title,labels \
      | jq '[.[] | select((.labels | map(.name) | any(startswith("blocked:")) | not) and (.labels | map(.name) | any(. == "in-progress") | not))]'
  SHELL
  take = "gh issue edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "epic" {
  source      = { queue = "epics" }
  handler     = { job = "epic" }
  concurrency = 2
}

job "epic" {
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

  step "implement" {
    run     = { agent = "implement" }
    on_done = { step = "submit" }
  }

  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      if test "$(git rev-list --count HEAD ^origin/${local.base})" -gt 0; then
        branch="${workspace.branch}" title="${local.title}"
        git push origin "$branch"
        gh issue close ${var.epic.number}
        # Unblock dependents
        gh issue list --label "blocked:${var.epic.number}" --state open --json number -q '.[].number' \
          | while read -r num; do gh issue edit "$num" --remove-label "blocked:${var.epic.number}"; done
        oj queue push merges --var branch="$branch" --var title="$title"
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
# Agents
# ------------------------------------------------------------------------------

agent "plan" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools EnterPlanMode,ExitPlanMode"
  on_dead = { action = "gate", run = "test \"$(gh issue view ${var.epic.number} --json comments -q '.comments | length')\" -gt 0" }

  session "tmux" {
    color = "blue"
    title = "Plan: #${var.epic.number}"
    status { left = "#${var.epic.number}: ${var.epic.title}" }
  }

  prime = ["gh issue view ${var.epic.number}"]

  prompt = <<-PROMPT
    Create an implementation plan for: #${var.epic.number} - ${var.epic.title}

    1. Spawn 3-5 Explore agents in parallel (depending on complexity)
    2. Spawn a Plan agent to synthesize findings
    3. Add the plan as a comment: `gh issue comment ${var.epic.number} -b "the plan"`
  PROMPT
}

agent "implement" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools EnterPlanMode,ExitPlanMode"
  on_dead = { action = "gate", run = "make check" }

  on_idle {
    action  = "nudge"
    message = <<-MSG
      Follow the plan, implement, test, then verify with:
      ```
      make check
      ```
      Then commit your changes.
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

  prime = ["gh issue view ${var.epic.number} --comments"]

  prompt = <<-PROMPT
    Implement: #${var.epic.number} - ${var.epic.title}

    The plan is in the issue comments above.

    1. Follow the plan
    2. Implement
    3. Verify: `make check`
    4. Commit
  PROMPT
}
