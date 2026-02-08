# GitHub PR CI/CD resolution.
#
# PRs are created with `merge:auto` and `gh pr merge --squash --auto`.
# GitHub CI runs checks and merges automatically when they pass.
#
# A cron polls for stuck PRs — those with failing checks or merge
# conflicts — and relabels them `merge:cicd` for agent-assisted
# resolution.

# ------------------------------------------------------------------------------
# Cron: detect stuck merge:auto PRs and escalate to cicd
# ------------------------------------------------------------------------------

cron "merge" {
  interval = "60s"
  run      = { job = "merge-check" }
}

job "merge-check" {
  name = "merge-check"

  step "scan" {
    run = <<-SHELL
      gh pr list --label merge:auto --json number,mergeable,statusCheckRollup --jq '
        .[] | select(
          .mergeable == "CONFLICTING" or
          (.statusCheckRollup | length > 0 and (map(select(.conclusion != "")) | length > 0) and any(.conclusion != "SUCCESS" and .conclusion != "NEUTRAL" and .conclusion != "SKIPPED" and .conclusion != ""))
        ) | .number
      ' | while read -r num; do
        echo "Escalating PR #$num to cicd"
        gh pr edit "$num" --remove-label merge:auto --add-label merge:cicd
      done
      oj worker start cicd 2>/dev/null || true
    SHELL
  }
}

# ------------------------------------------------------------------------------
# CI/CD resolve queue (agent-assisted resolution)
# ------------------------------------------------------------------------------

queue "cicd" {
  type = "external"
  list = "gh pr list --label merge:cicd --json number,title,headRefName --search '-label:in-progress'"
  take = "gh pr edit ${item.number} --add-label in-progress"
  poll = "30s"
}

worker "cicd" {
  source      = { queue = "cicd" }
  handler     = { job = "cicd" }
  concurrency = 1
}

job "cicd" {
  name      = "Resolve PR #${var.pr.number}: ${var.pr.title}"
  vars      = ["pr"]
  workspace = "folder"
  on_cancel = { step = "cleanup" }

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "merge-pr-${var.pr.number}-${workspace.nonce}"
  }

  notify {
    on_start = "Resolving PR #${var.pr.number}: ${var.pr.title}"
    on_done  = "Resolved PR #${var.pr.number}: ${var.pr.title}"
    on_fail  = "Resolve failed PR #${var.pr.number}: ${var.pr.title}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      git -C "${local.repo}" fetch origin main
      git -C "${local.repo}" fetch origin pull/${var.pr.number}/head:${local.branch}
      git -C "${local.repo}" worktree add "${workspace.root}" ${local.branch}
    SHELL
    on_done = { step = "resolve" }
  }

  step "resolve" {
    run     = { agent = "merge-resolver" }
    on_done = { step = "push" }
  }

  step "push" {
    run = <<-SHELL
      git push --force-with-lease origin HEAD:${var.pr.headRefName}
      gh pr edit ${var.pr.number} --remove-label merge:cicd,in-progress --add-label merge:auto
      gh pr merge ${var.pr.number} --squash --auto
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
# Agent
# ------------------------------------------------------------------------------

agent "merge-resolver" {
  run     = "claude --model opus --dangerously-skip-permissions"
  on_idle {
    action  = "nudge"
    message = <<-MSG
      CI is still failing or the PR hasn't merged yet.
      Check `gh pr checks ${var.pr.number}` and fix any remaining failures.
      Verify locally with `make check` before pushing.
    MSG
  }
  on_dead = { action = "escalate" }

  session "tmux" {
    color = "yellow"
    title = "Merge: PR #${var.pr.number}"
    status {
      left  = "${var.pr.title}"
      right = "${var.pr.headRefName} -> main"
    }
  }

  prime = [
    "echo '## Git Status'",
    "git status",
    "echo '## PR'",
    "gh pr view ${var.pr.number}",
    "echo '## PR Checks'",
    "gh pr checks ${var.pr.number} || true",
    "echo '## Commits (branch vs main)'",
    "git log --oneline origin/main..HEAD 2>/dev/null || true",
  ]

  prompt = <<-PROMPT
    You are fixing PR #${var.pr.number} ("${var.pr.title}") so it can merge into main.

    GitHub auto-merge is stuck — either CI is failing or there are conflicts.
    Check the PR status and build output above to diagnose the issue.

    If there are merge conflicts: rebase onto main, resolve conflicts, force-push.
    If CI is failing: fix the code, commit, push.

    Verify locally with `make check` before pushing.
  PROMPT
}
