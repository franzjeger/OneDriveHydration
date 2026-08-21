#!/usr/bin/env bash
set -euo pipefail

lock_file="${1:-Cargo.lock}"
packages=(hydration-client hydration-graph hydration-protocol)
resolved=""

for package in "${packages[@]}"; do
    revision="$({
        awk -v wanted="$package" '
            /^\[\[package\]\]$/ { in_package = 0 }
            $0 == "name = \"" wanted "\"" { in_package = 1; next }
            in_package && /^source = "git\+https:\/\/github\.com\/franzjeger\/HydrationAPI/ {
                source = $0
                sub(/^.*#/, "", source)
                sub(/"$/, "", source)
                print source
                exit
            }
        ' "$lock_file"
    } || true)"

    if [[ ! "$revision" =~ ^[0-9a-f]{40}$ ]]; then
        echo "could not resolve $package to a full HydrationAPI commit in $lock_file" >&2
        exit 1
    fi
    if [[ -n "$resolved" && "$revision" != "$resolved" ]]; then
        echo "HydrationAPI packages resolve to different commits in $lock_file" >&2
        echo "expected $resolved, but $package resolves to $revision" >&2
        exit 1
    fi
    resolved="$revision"
done

all_revisions="$(
    awk '
        /^source = "git\+https:\/\/github\.com\/franzjeger\/HydrationAPI/ {
            source = $0
            sub(/^.*#/, "", source)
            sub(/"$/, "", source)
            print source
        }
    ' "$lock_file" | sort -u
)"
if [[ "$all_revisions" != "$resolved" ]]; then
    echo "not every HydrationAPI package resolves to $resolved in $lock_file" >&2
    printf '%s\n' "$all_revisions" >&2
    exit 1
fi

printf '%s\n' "$resolved"
