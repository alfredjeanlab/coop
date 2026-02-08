# GitHub Issues as an epic queue with planning and implementation.
#
# Workflow: create epic issue → plan worker explores and writes plan →
# build worker implements the plan → PR with auto-merge.

# Create a new epic with 'plan:needed' and 'build:needed'.
#
# Examples:
#   oj run epic "Implement user authentication with OAuth"
#   oj run epic "Implement OAuth" "Support Google and GitHub providers"
#   oj run epic "Refactor storage layer" --blocked 5
#   oj run epic "Wire everything together" --blocked "3 5 14"
command "github:epic" {
  args = "<title> [body] [--blocked <numbers>]"
  run  = <<-SHELL
    labels="type:epic,plan:needed,build:needed"
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
      url=$(gh issue create --label "$labels" --title "${args.title}" --body "$body")
    else
      url=$(gh issue create --label "$labels" --title "${args.title}")
    fi
    gh issue lock "$${url##*/}"
    oj worker start plan
    oj worker start epic
  SHELL

  defaults = {
    body    = ""
    blocked = ""
  }
}

# Create a new epic with 'plan:needed' only (no auto-build).
#
# Examples:
#   oj run idea "Add caching layer for API responses"
#   oj run idea "Prototype new UI layout" "Explore grid vs flex"
command "github:idea" {
  args = "<title> [body]"
  run  = <<-SHELL
    if [ -n "${args.body}" ]; then
      url=$(gh issue create --label type:epic,plan:needed --title "${args.title}" --body "${args.body}")
    else
      url=$(gh issue create --label type:epic,plan:needed --title "${args.title}")
    fi
    gh issue lock "$${url##*/}"
    oj worker start plan
  SHELL

  defaults = {
    body = ""
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
    oj worker start plan
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
    oj worker start epic
  SHELL
}

# ------------------------------------------------------------------------------
# Plan queue and worker
# ------------------------------------------------------------------------------

queue "plans" {
  type = "external"
  list = "gh issue list --label type:epic,plan:needed --state open --json number,title --search '-label:blocked -label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress && gh issue lock ${item.number}"
  poll = "30s"
}

worker "plan" {
  source      = { queue = "plans" }
  handler     = { job = "plan" }
  concurrency = 5
}

job "plan" {
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
  list = "gh issue list --label type:epic,plan:ready,build:needed --state open --json number,title --search '-label:blocked -label:in-progress'"
  take = "gh issue edit ${item.number} --add-label in-progress && gh issue lock ${item.number}"
  poll = "30s"
}

worker "epic" {
  source      = { queue = "epics" }
  handler     = { job = "epic" }
  concurrency = 5
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

  notify {
    on_start = "Building: ${var.epic.title}"
    on_done  = "Built: ${var.epic.title}"
    on_fail  = "Build failed: ${var.epic.title}"
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
        gh pr merge --squash --auto
        gh issue edit ${var.epic.number} --remove-label build:needed,in-progress --add-label build:ready
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
  on_dead = { action = "resume", attempts = 1 }

  session "tmux" {
    color = "blue"
    title = "Plan: #${var.epic.number}"
    status { left = "#${var.epic.number}: ${var.epic.title}" }
  }

  prime = [
    "gh issue view ${var.epic.number}",
    <<-PRIME
    echo '## Workflow'
    echo
    echo '1. Spawn 3-5 Explore agents in parallel to understand the codebase'
    echo '2. Spawn a Plan agent to synthesize findings into a plan'
    echo '3. Add the plan as a comment: `gh issue comment ${var.epic.number} -b "the plan"`'
    echo
    echo 'The job will not advance until a comment is added to the issue.'
    PRIME
  ]

  prompt = "Create an implementation plan for GitHub issue #${var.epic.number}: ${var.epic.title}"
}

agent "implement" {
  run     = "claude --model opus --dangerously-skip-permissions --disallowed-tools EnterPlanMode,ExitPlanMode"
  on_dead = { action = "resume", attempts = 1 }

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

  prime = [
    "gh issue view ${var.epic.number} --comments",
    <<-PRIME
    echo '## Workflow'
    echo
    echo 'The plan is in the issue comments above.'
    echo
    echo '1. Follow the plan and implement the changes'
    echo '2. Write or update tests'
    echo '3. Verify: `make check` — changes REJECTED if this fails'
    echo '4. Commit your changes'
    PRIME
  ]

  prompt = "Implement GitHub issue #${var.epic.number}: ${var.epic.title}"
}
