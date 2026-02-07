# GitHub-backed epic queue with planning and implementation.
#
# Workflow: create epic issue → plan worker explores and writes plan →
# epic worker implements the plan → merge queue.
#
# Blocking: single `blocked` label. Dependencies tracked in issue body
# as "Blocked by: #2, #5, #14". The submit step calls `unblock` to
# check all blocked issues and remove the label when deps resolve.

# Create a new epic with 'plan:needed' and 'build:needed'.
#
# Examples:
#   oj run epic "Implement user authentication with OAuth"
#   oj run epic "Refactor storage layer" --after 5
#   oj run epic "Wire everything together" --after "3 5 14"
command "github:epic" {
  args = "<description> [--after <numbers>]"
  run  = <<-SHELL
    labels="type:epic,plan:needed,build:needed"
    body=""
    if [ -n "${args.after}" ]; then
      labels="$labels,blocked"
      refs=""
      for n in ${args.after}; do refs="$refs #$n"; done
      body="Blocked by:$refs"
    fi
    if [ -n "$body" ]; then
      gh issue create --label "$labels" --title "${args.description}" --body "$body"
    else
      gh issue create --label "$labels" --title "${args.description}"
    fi
    oj worker start github:plan
    oj worker start github:build
  SHELL

  defaults = {
    after = ""
  }
}

# Create a new epic with 'plan:needed' only (no auto-build).
#
# Examples:
#   oj run idea "Add caching layer for API responses"
#   oj run idea "Add caching layer" --after "3 5"
command "idea" {
  args = "<description> [--after <numbers>]"
  run  = <<-SHELL
    labels="type:epic,plan:needed"
    body=""
    if [ -n "${args.after}" ]; then
      labels="$labels,blocked"
      refs=""
      for n in ${args.after}; do refs="$refs #$n"; done
      body="Blocked by:$refs"
    fi
    if [ -n "$body" ]; then
      gh issue create --label "$labels" --title "${args.description}" --body "$body"
    else
      gh issue create --label "$labels" --title "${args.description}"
    fi
    oj worker start github:plan
  SHELL

  defaults = {
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
#   oj run unblock
command "unblock" {
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
        oj run unblock
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
