# Dependency blocking for GitHub Issues.
#
# Issues can declare dependencies via "Blocked by: #2, #5, #14" in the body.
# The `blocked` label prevents workers from picking up the issue. An unblock
# cron runs every 60s to detect when all deps close and removes the label.
#
# A dep counts as resolved only if CLOSED without pipeline-pending labels
# (build:failed, plan:failed, build:needed, plan:needed). This prevents
# premature unblocking when a dep is closed by a cancelled job or has a
# failed build.

# Check all blocked issues and remove label when all deps are resolved.
#
# Examples:
#   oj run unblock
command "github:unblock" {
  run = <<-SHELL
    gh issue list --label blocked --state open --json number,body | jq -c '.[]' | while read -r obj; do
      num=$(echo "$obj" | jq -r .number)
      deps=$(echo "$obj" | jq -r '.body' | grep -iE 'Blocked by:?' | grep -oE '#[0-9]+' | grep -oE '[0-9]+' || true)
      if [ -z "$deps" ]; then
        gh issue edit "$num" --remove-label blocked
        echo "Unblocked #$num (no deps)"
        continue
      fi
      all_resolved=true
      for dep in $deps; do
        dep_data=$(gh issue view "$dep" --json state,labels 2>/dev/null)
        state=$(echo "$dep_data" | jq -r .state)
        if [ "$state" != "CLOSED" ]; then
          all_resolved=false
          break
        fi
        pending=$(echo "$dep_data" | jq '[.labels[].name | select(test("^(build:failed|plan:failed|build:needed|plan:needed)$"))] | length')
        if [ "$pending" -gt 0 ]; then
          all_resolved=false
          break
        fi
      done
      if [ "$all_resolved" = true ]; then
        gh issue edit "$num" --remove-label blocked
        echo "Unblocked #$num"
      fi
    done
  SHELL
}

cron "unblock" {
  interval = "60s"
  run      = { job = "unblock" }
}

job "unblock" {
  name = "unblock"

  step "check" {
    run = <<-SHELL
      gh issue list --label blocked --state open --json number,body | jq -c '.[]' | while read -r obj; do
        num=$(echo "$obj" | jq -r .number)
        deps=$(echo "$obj" | jq -r '.body' | grep -iE 'Blocked by:?' | grep -oE '#[0-9]+' | grep -oE '[0-9]+' || true)
        if [ -z "$deps" ]; then
          gh issue edit "$num" --remove-label blocked
          echo "Unblocked #$num (no deps)"
          continue
        fi
        all_resolved=true
        for dep in $deps; do
          dep_data=$(gh issue view "$dep" --json state,labels 2>/dev/null)
          state=$(echo "$dep_data" | jq -r .state)
          if [ "$state" != "CLOSED" ]; then
            all_resolved=false
            break
          fi
          pending=$(echo "$dep_data" | jq '[.labels[].name | select(test("^(build:failed|plan:failed|build:needed|plan:needed)$"))] | length')
          if [ "$pending" -gt 0 ]; then
            all_resolved=false
            break
          fi
        done
        if [ "$all_resolved" = true ]; then
          gh issue edit "$num" --remove-label blocked
          echo "Unblocked #$num"
        fi
      done
    SHELL
  }
}
