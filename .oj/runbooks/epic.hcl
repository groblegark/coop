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

# Idempotently create all GitHub labels used by this runbook.
#
# Examples:
#   oj run github:setup
command "github:setup" {
  run = <<-SHELL
    gh label create "type:epic"      --color 5319E7 --description "Epic feature issue"       --force
    gh label create "plan:needed"    --color FBCA04 --description "Needs implementation plan" --force
    gh label create "plan:ready"     --color 0E8A16 --description "Plan complete"             --force
    gh label create "plan:failed"    --color D93F0B --description "Planning failed"           --force
    gh label create "build:needed"   --color FBCA04 --description "Needs implementation"      --force
    gh label create "build:ready"    --color 0E8A16 --description "Built, PR awaiting merge"  --force
    gh label create "build:failed"   --color D93F0B --description "Build failed"              --force
    gh label create "blocked"        --color B60205 --description "Blocked by dependencies"   --force
    gh label create "in-progress"    --color 1D76DB --description "Work in progress"          --force
    gh label create "auto-merge"     --color 0E8A16 --description "PR queued for auto-merge"  --force
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
        branch="${workspace.branch}"
        git push origin "$branch"
        gh pr create --title "${local.title}" --body "Closes #${var.epic.number}" --head "$branch" --label auto-merge
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
# Merge queue and worker
# ------------------------------------------------------------------------------

queue "github:merges" {
  type = "external"
  list = "gh pr list --label auto-merge --json number,title,headRefName"
  take = "gh pr edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "github:merge" {
  source      = { queue = "github:merges" }
  handler     = { job = "github:merge" }
  concurrency = 1
}

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
    on_done = { step = "push" }
  }

  step "push" {
    run = <<-SHELL
      git push --force-with-lease origin HEAD:${var.pr.headRefName}
      gh pr merge ${var.pr.number} --squash --auto
      gh pr edit ${var.pr.number} --remove-label in-progress
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
  on_dead = { action = "gate", run = "test \"$(gh issue view ${var.epic.number} --json comments -q '.comments | length')\" -gt 0" }

  session "tmux" {
    color = "blue"
    title = "Plan: #${var.epic.number}"
    status { left = "#${var.epic.number}: ${var.epic.title}" }
  }

  prime = ["gh issue view ${var.epic.number}"]

  prompt = <<-PROMPT
    Create an implementation plan for GitHub issue #${var.epic.number}: ${var.epic.title}

    The issue details are above (from `gh issue view ${var.epic.number}`).

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
    Implement GitHub issue #${var.epic.number}: ${var.epic.title}

    The plan is in the issue comments above (from `gh issue view ${var.epic.number} --comments`).

    1. Follow the plan
    2. Implement
    3. Verify: `make check`
    4. Commit
  PROMPT
}
