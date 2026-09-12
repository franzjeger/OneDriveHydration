#!/bin/sh
# Dispatch a mixed selection without parsing filenames as shell code.
set -u
here=$(dirname "$(readlink -f "$0")")
verb=$1
shift
case "$verb" in keep|free) ;; *) exit 2;; esac
result=0
for path in "$@"; do
    if [ "$verb" = keep ]; then
        "$here/keep-on-device.sh" "$path" || result=1
    elif [ -d "$path" ]; then
        "$here/free-up-space-folder.sh" "$path" || result=1
    else
        "$here/free-up-space.sh" "$path" || result=1
    fi
done
exit "$result"
