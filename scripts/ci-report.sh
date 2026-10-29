#!/usr/bin/env bash
# Posts a build-check summary (and the tail of every failed step's log) as a comment on the commit
# that was just built, so the person or agent who pushed it can read compile errors without
# opening the Actions UI. Used by .github/workflows/claude-build-check.yml.
#
#   scripts/ci-report.sh <job-name> "<label>|<outcome>|<logfile>" ...
set -u
job="$1"; shift
body="$(mktemp)"
{
  echo "### build check · ${job} · ${GITHUB_REF_NAME:-?} @ ${GITHUB_SHA:0:7}"
  echo
} > "$body"
for spec in "$@"; do
  IFS='|' read -r label outcome _ <<<"$spec"
  echo "- **${label}**: ${outcome}" >> "$body"
done
for spec in "$@"; do
  IFS='|' read -r label outcome log <<<"$spec"
  if [ "$outcome" != success ] && [ -f "$log" ]; then
    {
      echo
      echo "<details open><summary>${label}: log tail</summary>"
      echo
      echo '```'
      grep -vE '^\s+(Compiling|Checking|Downloaded|Downloading|Fresh|Updating|Locking|Adding|Blocking|Compiled)' "$log" \
        | tail -n 300 | tail -c 30000
      echo '```'
      echo "</details>"
    } >> "$body"
  fi
done
gh api "repos/${GITHUB_REPOSITORY}/commits/${GITHUB_SHA}/comments" -f body="$(cat "$body")" >/dev/null \
  || echo "could not post the commit comment"
