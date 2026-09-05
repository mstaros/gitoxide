#!/usr/bin/env bash
set -eu -o pipefail

# Bisects that are actually stepping, which `make_bisect_repo.sh` is not: a bare
# `git bisect start` leaves `HEAD` symbolic, and `insert_head` only consults the
# holding logic once `HEAD` is detached. Supplying both good and bad makes git check
# out a midpoint, which is the state a branch is held in.
#
# The two repositories differ only in what `HEAD` was when the bisect began.
# `builtin/bisect.c:940-948` writes the short branch name into `BISECT_START` when
# `HEAD` is symbolic, and the full object id when it is detached. That is the whole
# distinction: the first holds `refs/heads/topic`, the second holds nothing.
#
# Good and bad are resolved before `git bisect start`, because the first
# `git bisect bad` checks out a midpoint and would move `HEAD~4` underneath us.

mkdir started-on-branch
(cd started-on-branch
  git init -q
  for n in 1 2 3 4 5; do
    echo "$n" > f
    git add f
    git commit -q -m "$n"
  done

  git checkout -q -b topic
  bad="$(git rev-parse HEAD)"
  good="$(git rev-parse HEAD~4)"

  git bisect start
  git bisect bad "$bad"
  git bisect good "$good"
)

mkdir started-detached
(cd started-detached
  git init -q
  for n in 1 2 3 4 5; do
    echo "$n" > f
    git add f
    git commit -q -m "$n"
  done

  git checkout -q --detach HEAD
  bad="$(git rev-parse HEAD)"
  good="$(git rev-parse HEAD~4)"

  git bisect start
  git bisect bad "$bad"
  git bisect good "$good"
)
