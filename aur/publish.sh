#!/usr/bin/env bash
set -euo pipefail

echo "==> Testing SSH connection to AUR..."
if ! ssh -T aur@aur.archlinux.org 2>&1 | grep -q "Interactive shell is disabled"; then
    echo "Error: SSH authentication to aur@aur.archlinux.org failed."
    echo "Make sure your public key (~/.ssh/aur.pub) is added to your AUR account at https://aur.archlinux.org/account/."
    exit 1
fi

AUR_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="$(mktemp -d /tmp/aur-publish-XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT

for pkg in legendary-tui legendary-tui-git; do
    echo "==> Publishing $pkg to AUR..."
    if git clone "ssh://aur@aur.archlinux.org/$pkg.git" "$WORK_DIR/$pkg" 2>/dev/null; then
        cd "$WORK_DIR/$pkg"
    else
        echo "Initializing new AUR repository for $pkg..."
        mkdir -p "$WORK_DIR/$pkg"
        cd "$WORK_DIR/$pkg"
        git init -b master
        git remote add origin "ssh://aur@aur.archlinux.org/$pkg.git"
    fi
    cp "$AUR_DIR/$pkg/PKGBUILD" "$WORK_DIR/$pkg/"
    cp "$AUR_DIR/$pkg/.SRCINFO" "$WORK_DIR/$pkg/"
    git add PKGBUILD .SRCINFO
    if git diff --staged --quiet; then
        echo "No changes for $pkg."
    else
        git commit -m "release: update $pkg to 1.0.1"
        git push -u origin master
        echo "[✓] $pkg successfully published to AUR!"
    fi
done
