#!/usr/bin/env bash
set -eu -o pipefail

# A `git rebase --apply` stopped by a conflict, so `.git/rebase-apply/head-name` is
# present and names `refs/heads/topic`, and `HEAD` is detached onto the commit being
# replayed. Distinct from `make_rebase_i_repo.sh`, which produces `rebase-merge`.

mkdir rebase-apply-conflict
(cd rebase-apply-conflict
  git init -q

  echo base > f
  git add f
  git commit -q -m base

  git branch upstream
  git checkout -q -b topic

  echo topic > f
  git commit -q -am topic

  git checkout -q upstream
  echo upstream > f
  git commit -q -am upstream

  git checkout -q topic

  # Conflicts on purpose: the rebase stops and leaves its state directory behind.
  git rebase --apply upstream || true
)
