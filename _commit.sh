#!/usr/bin/env bash
set -uo pipefail
cd /c/Users/riezh/OneDrive/Documents/test/claude_core
rm -f _git.sh
git add -A
git reset -q _commit_msg.txt 2>/dev/null
git commit -F _commit_msg.txt 2>&1 | tail -6
rm -f _commit_msg.txt
echo "=== push ==="
git push origin main 2>&1 | tail -8
echo "=== head ==="
git log --oneline -1
