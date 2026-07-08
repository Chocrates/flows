#!/bin/sh
# Sum the integers 1..n. (There is a bug: it undercounts by n.)
sum_to() {
  n="$1"
  total=0
  i=1
  while [ "$i" -lt "$n" ]; do
    total=$((total + i))
    i=$((i + 1))
  done
  echo "$total"
}

sum_to "${1:-10}"
