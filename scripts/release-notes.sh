#!/bin/bash
# Print release notes for TAG: one bullet per commit since the previous tag, as
# "- **Subject.** First paragraph of the body". yamc shows these when you hover its update button.
set -euo pipefail
tag=$1
prev=$(git describe --tags --abbrev=0 "$tag^" 2>/dev/null || true)
git log --no-merges --format='%s%x1f%b%x1e' ${prev:+"$prev.."}"$tag" | python3 -c '
import sys
for entry in sys.stdin.read().split("\x1e"):
    if not entry.strip():
        continue
    subject, _, body = entry.strip().partition("\x1f")
    para = []
    for line in body.strip().splitlines():
        if not line.strip() or ":" in line.split(" ")[0]:  # blank line or a trailer like Co-Authored-By:
            break
        para.append(line.strip())
    text = " ".join(para)
    title = subject.rstrip(".")
    print(f"- **{title}.** {text}" if text else f"- {subject}")
'
if [[ -n $prev ]]; then
  echo
  echo "**Full Changelog**: https://github.com/krymancer/yamc/compare/$prev...$tag"
fi
