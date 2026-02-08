# Idempotently create all GitHub labels used by this runbook.
#
# Also recommended (configured manually or via GitHub settings):
#   - Branch protection on main requiring CI to pass
#   - strict=false: branches don't need to be up-to-date before merging
#   - Merge queue with squash merges so CI runs on the merged result
#
# Examples:
#   oj run github:setup
command "github:setup" {
  run = <<-SHELL
    gh label create "type:epic"      --color 5319E7 --description "Epic feature issue"       --force
    gh label create "type:chore"    --color FBCA04 --description "Maintenance task"          --force
    gh label create "type:bug"      --color D73A4A --description "Bug fix"                   --force
    gh label create "plan:needed"    --color FBCA04 --description "Needs implementation plan" --force
    gh label create "plan:ready"     --color 0E8A16 --description "Plan complete"             --force
    gh label create "plan:failed"    --color D93F0B --description "Planning failed"           --force
    gh label create "build:needed"   --color FBCA04 --description "Needs implementation"      --force
    gh label create "build:ready"    --color 0E8A16 --description "Built, PR awaiting merge"  --force
    gh label create "build:failed"   --color D93F0B --description "Build failed"              --force
    gh label create "blocked"        --color B60205 --description "Blocked by dependencies"   --force
    gh label create "in-progress"    --color 1D76DB --description "Work in progress"          --force
    gh label create "merge:auto"     --color 0E8A16 --description "PR queued for auto-merge"  --force
    gh label create "merge:cicd"     --color D93F0B --description "PR needs CI/CD resolution"  --force
  SHELL
}
