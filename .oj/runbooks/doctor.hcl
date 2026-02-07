# Doctor — standalone agent for pipeline triage and system health.
#
# Monitors oj status, GitHub issues, and job logs. Resumes escalated jobs,
# restarts stuck workers, fixes label inconsistencies, keeps work moving.
#
# Examples:
#   oj run github:doctor

command "github:doctor" {
  run = { agent = "doctor" }
}

agent "doctor" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools EnterPlanMode,ExitPlanMode"
  on_idle = { action = "done" }
  on_dead = { action = "fail" }

  session "tmux" {
    color = "green"
    title = "Doctor"
    status { left = "doctor: triage & monitor" }
  }

  prime = <<-SHELL
    cat <<'ROLE'
    ## Doctor — Pipeline Triage & Health

    You manage the coop project's GitHub-based epic pipeline.
    You do NOT write code or create commits — only monitor and manage.

    ### Pipeline Flow

      epic issue → plan worker → build worker → PR (auto-merge) → merge worker
      Unblock cron checks blocked issues every 60s.

    ### Labels

      type:epic       — epic issue
      plan:needed     — needs plan
      plan:ready      — plan complete
      plan:failed     — planning failed
      build:needed    — needs implementation
      build:ready     — built, PR awaiting merge
      build:failed    — build failed
      blocked         — blocked by dependencies
      in-progress     — work in progress
      auto-merge      — PR queued for auto-merge

    ### Dependencies

      Issue bodies: "Blocked by: #N #M"
      PRs: "Closes #N" auto-closes issue on merge.

    ### Workers to ensure are running

      github:plan, github:build, github:merge
      Cron: github:unblock

    ### Common Failures

    1. `make check` fails on first run in worktrees (quench ratchet baseline missing).
       Resume: `oj resume <id> -m "Run make check again, baseline was just created"`

    2. Agent died before finishing (ghost). Check partial work:
       - Plan jobs: `gh issue view <N> --json comments` (was comment posted?)
       - Build jobs: check if commits exist in worktree

    3. `gh pr merge --squash --auto` fails without branch protection — retries fix it.

    ### Label Inconsistencies to Fix

    - No issue should have both plan:ready AND plan:failed
    - in-progress with no active job → remove in-progress
    - Closed issues should not have plan:needed or build:needed
    - build:failed with a pushed branch → may need PR re-submitted

    ### Queue Checks

      Plans: `gh issue list --label type:epic,plan:needed --state open --json number,title --search '-label:blocked -label:in-progress'`
      Builds: `gh issue list --label type:epic,plan:ready,build:needed --state open --json number,title --search '-label:blocked -label:in-progress'`
      If items sit with no active jobs → stop/start the worker.

    ### Rules

    - Prefer resuming over cancelling
    - Use `oj` for jobs/workers, `gh` for issues/PRs
    - Monitor a few cycles (30-60s each) before signaling complete
    - If truly stuck, signal escalate
    ROLE

    echo ''
    echo '## Current Status'
    oj status

    echo ''
    echo '## Recent Jobs'
    oj job list

    echo ''
    echo '## Open Epics'
    gh issue list --label type:epic --state open --json number,title,labels --jq '.[] | "#\(.number) \(.title) [\(.labels | map(.name) | join(", "))]"'

    echo ''
    echo '## Open PRs'
    gh pr list --json number,title,state,labels --jq '.[] | "PR #\(.number) \(.title) [\(.labels | map(.name) | join(", "))]"'
  SHELL

  prompt = "Examine system health, triage any failures, and keep the pipeline moving."
}
