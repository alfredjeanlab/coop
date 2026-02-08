# GitHub Issues as a chore queue with PR-based merge.

# File a GitHub chore and dispatch it to a worker.
#
# Examples:
#   oj run chore "Update dependencies to latest versions"
#   oj run chore "Add missing test coverage for auth module" "Details here..."
#   oj run chore "Cleanup after migration" --blocked 15
command "github:chore" {
  args = "<title> [body] [--blocked <numbers>]"
  run  = <<-SHELL
    labels="type:chore"
    body="${args.body}"
    _blocked="${args.blocked}"
    if [ -n "$_blocked" ]; then
      labels="$labels,blocked"
      nums=$(echo "$_blocked" | tr ',' ' ')
      refs=""
      for n in $nums; do refs="$refs #$n"; done
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
    oj worker start chore
  SHELL

  defaults = {
    body    = ""
    blocked = ""
  }
}

queue "chores" {
  type = "external"
  list = "gh issue list --label type:chore --state open --json number,title --search '-label:blocked -label:in-progress'"
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
        gh pr merge --squash --auto
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

  prime = [
    "gh issue view ${var.task.number}",
    <<-PRIME
    echo '## Workflow'
    echo
    echo '1. Understand the task and find the relevant code'
    echo '2. Implement the changes and write or update tests'
    echo '3. Verify: `make check` — changes REJECTED if this fails'
    echo '4. Commit your changes'
    echo
    echo 'If already completed by a prior commit, just commit a no-op.'
    PRIME
  ]

  prompt = "Complete GitHub issue #${var.task.number}: ${var.task.title}"
}
