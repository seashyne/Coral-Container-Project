#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

git remote add origin https://github.com/seashyne/Coral-Container-Project.git 2>/dev/null || true

echo "Pushing Coral main branch to GitHub..."
git push -u origin main
