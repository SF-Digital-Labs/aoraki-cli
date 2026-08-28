/// The server-side post-receive hook, installed into each bare repo by
/// `aoraki link`. It reads its settings from `git config aoraki.*` in the
/// bare repo, so the same script works for every app/box.
///
/// Design notes:
/// - Detached checkout of the exact pushed SHA — no branch state on the box
///   that can diverge (the classic `ucn` failure mode).
/// - The build runs under `nohup` with output tee'd to a log file: if the
///   client disconnects mid-deploy, the build finishes server-side and the
///   outcome lands in the deploy log.
/// - The final "aoraki: deploy status: ..." line is a sentinel the CLI
///   scans for, because git push exit codes don't reflect hook failures.
pub const HOOK_VERSION: u32 = 1;

const HOOK_TEMPLATE: &str = r##"#!/usr/bin/env bash
# aoraki-cli post-receive hook — installed by `aoraki link`; do not edit by hand.
# aoraki-hook-version: __VERSION__
set -uo pipefail
unset GIT_DIR GIT_WORK_TREE

BARE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cfg() { git --git-dir="$BARE_DIR" config --get "aoraki.$1"; }

APP="$(cfg app)" || { echo "aoraki: hook not configured (aoraki.app missing) — re-run 'aoraki link'" >&2; exit 1; }
BRANCH="$(cfg branch)"
WORKDIR="$(cfg workdir)"
DEPLOY_SCRIPT="$(cfg deployscript)"

LOG_DIR="/data/logs/aoraki-deploys"
if ! mkdir -p "$LOG_DIR" 2>/dev/null; then
  LOG_DIR="$BARE_DIR/deploy-logs"
  mkdir -p "$LOG_DIR"
fi

record() {
  printf '{"app":"%s","branch":"%s","sha":"%s","status":"%s","time":"%s"%s}\n' \
    "$APP" "$BRANCH" "$NEW" "$1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$2" >> "$LOG_DIR/$APP.jsonl"
}

DEPLOY_STATUS=0
while read -r OLD NEW REF; do
  if [ "$REF" != "refs/heads/$BRANCH" ]; then
    echo "aoraki: ignoring $REF (this box deploys refs/heads/$BRANCH)"
    continue
  fi
  SHORT="${NEW:0:7}"

  exec 9>"$LOG_DIR/$APP.lock"
  if ! flock -n 9; then
    echo "aoraki: another deploy of $APP is already running — aborting"
    echo "aoraki: deploy status: failed"
    DEPLOY_STATUS=1
    continue
  fi

  START_S="$(date +%s)"
  record started ""
  echo "aoraki: checking out $SHORT into $WORKDIR"
  if ! git -C "$WORKDIR" fetch -q "$BARE_DIR" "$BRANCH" || ! git -C "$WORKDIR" checkout -q -f "$NEW"; then
    echo "aoraki: checkout failed"
    record failed ',"reason":"checkout"'
    echo "aoraki: deploy status: failed"
    DEPLOY_STATUS=1
    continue
  fi

  RUN_LOG="$LOG_DIR/$APP-$SHORT-$START_S.log"
  STATUS_FILE="$RUN_LOG.status"
  rm -f "$STATUS_FILE"

  echo "aoraki: running $DEPLOY_SCRIPT (BUILD_HASH=$SHORT)"
  nohup bash -c "cd '$WORKDIR' && BUILD_HASH='$SHORT' bash '$DEPLOY_SCRIPT'; echo \$? > '$STATUS_FILE'" \
    > "$RUN_LOG" 2>&1 < /dev/null &
  BUILD_PID=$!

  tail --pid="$BUILD_PID" -n +1 -f "$RUN_LOG" 2>/dev/null
  wait "$BUILD_PID" 2>/dev/null

  EXIT_CODE="$(cat "$STATUS_FILE" 2>/dev/null || echo 1)"
  DURATION=$(( $(date +%s) - START_S ))

  if [ "$EXIT_CODE" = "0" ]; then
    record succeeded ",\"duration_secs\":$DURATION"
    echo "aoraki: deploy status: succeeded (${DURATION}s)"
  else
    record failed ",\"duration_secs\":$DURATION,\"exit_code\":$EXIT_CODE"
    echo "aoraki: deploy status: failed (exit $EXIT_CODE after ${DURATION}s)"
    DEPLOY_STATUS=1
  fi
done

exit $DEPLOY_STATUS
"##;

pub fn script() -> String {
    HOOK_TEMPLATE.replace("__VERSION__", &HOOK_VERSION.to_string())
}
