#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# scripts/leftovers.sh: the T22.7 leftover audit as a runnable check.
#
# Every "Not done:" line in done.md must sit in at least one of three sets:
# closed by a done task, owned by an open plan.md §3 card, or recorded as a
# deliberate remainder in docs/compat.md ("Known leftovers", one sentence why).
# Anything in none of the three fails the build, so a new leftover cannot
# slip in silently with its task card.
set -euo pipefail
# Anchor on the repo root so `just check` and a direct call agree on paths.
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Done tasks whose landing closed the leftover. P22 closed the trust half;
# the rest died in their own follow-ups (T17.x, T9.x, T15.x, …), which is why
# this table names the follow-up rather than pretending P22 did everything.
closed_by() {
    case "$1" in
        T1.6) echo "T17.4 T17.5" ;;
        T2.2) echo "T17.1" ;;
        T2.6) echo "T2.6" ;;
        T3.7) echo "T9.2 T4.1 T4.2" ;;
        T3.8) echo "" ;;
        T3.9) echo "T9.2 T9.3" ;;
        T4.1) echo "T4.2 T4.3 T5.8" ;;
        T4.2) echo "T4.3" ;;
        T4.3) echo "T6.1 T5.1" ;;
        T4.4) echo "" ;;
        T5.1) echo "T5.8 T5.5" ;;
        T5.2) echo "T22.2 T5.5 T5.3" ;;
        T5.3) echo "T24.4" ;;
        T5.4) echo "T5.6 T24.4" ;;
        T5.5) echo "T5.7 T8.1" ;;
        T5.6) echo "T5.8 T24.4" ;;
        T5.7) echo "T25.4" ;;
        T5.8) echo "T1.3 T22.1" ;;
        T6.1) echo "T17.2 T6.3" ;;
        T6.2) echo "T3.2" ;;
        T7.1) echo "T22.2" ;;
        T7.2) echo "T22.2" ;;
        T7.3) echo "T22.2 T9.3" ;;
        T7.4) echo "T22.3 T7.5 T16.2" ;;
        T7.6) echo "T22.5" ;;
        T14.1) echo "" ;;
        T15.1) echo "T15.2 T15.3 T15.4" ;;
        T14.2) echo "T14.3 T22.6 T24.2" ;;
        T17.3) echo "T18.1" ;;
        *) echo "" ;;
    esac
}

# Open plan.md §3 cards that still own the leftover (currently only the
# OSC 52 clipboard card: Cmd::Copy is still a no-op until T23.4 lands it).
open_task() {
    case "$1" in
        T5.1 | T5.3 | T5.5) echo "T23.4" ;;
        *) echo "" ;;
    esac
}

# Leftovers that stay deliberately; each needs a row in docs/compat.md.
stays() {
    case "$1" in
        T1.6 | T2.2 | T3.7 | T3.8 | T3.9 | T4.2 | T4.4 | T5.3 | T5.4 | T5.5 | \
            T5.6 | T5.7 | T6.1 | T6.2 | T6.3 | T7.1 | T7.2 | T7.3 | T7.4 | T7.5 | \
            T7.6 | T14.1 | T15.1 | T14.2 | T14.3 | T18.1) return 0 ;;
        *) return 1 ;;
    esac
}

# The full mapping key set, so a mapping without a leftover fails too —
# otherwise a fixed leftover would leave a dead entry nobody re-checks.
mapped_ids() {
    echo "T1.6 T2.2 T2.6 T3.7 T3.8 T3.9 T4.1 T4.2 T4.3 T4.4 T5.1 T5.2 T5.3 T5.4 T5.5 T5.6 T5.7 T5.8 T6.1 T6.2 T6.3 T7.1 T7.2 T7.3 T7.4 T7.5 T7.6 T14.1 T15.1 T14.2 T14.3 T17.3 T18.1"
}

fail=0

# Each "Not done:" line with the task id from its nearest card header above.
items=$(awk '/^#### T[0-9]+\.[0-9]+/ { if (match($0, /T[0-9]+\.[0-9]+/)) cur = substr($0, RSTART, RLENGTH) } /Not done:/ { print cur "|" FNR }' done.md)
# A renamed marker must fail loudly, never pass vacuously on zero items.
if [ -z "$items" ]; then
    echo "leftovers: no 'Not done:' lines extracted from done.md; the marker or headers moved" >&2
    exit 1
fi

# Task ids owned by the compat table: the first column of its rows only, so a
# "why it stays" sentence naming another card (T23.4, T22.5) cannot satisfy it.
compat_ids=$(awk '/^## Known leftovers/ { on = 1; next } /^## / { on = 0 } on && /^\| T[0-9]+\.[0-9]+ \|/ { gsub(/^[| ]+/, ""); gsub(/ \|.*/, ""); print }' docs/compat.md | sort -u || true)
if [ -z "$compat_ids" ]; then
    echo "leftovers: docs/compat.md has no '## Known leftovers' section" >&2
    exit 1
fi

count=0
while IFS='|' read -r id line; do
    count=$((count + 1))
    if [ -z "$id" ]; then
        echo "leftovers: done.md:${line}: 'Not done:' without a task header above it" >&2
        fail=1
        continue
    fi
    closed=$(closed_by "$id")
    open=$(open_task "$id")
    stay="no"
    stays "$id" && stay="yes"
    if [ -z "$closed" ] && [ -z "$open" ] && [ "$stay" = "no" ]; then
        echo "leftovers: ${id} (done.md:${line}) is in none of the three sets: no closing task, no §3 card, no compat row" >&2
        fail=1
        continue
    fi
    for ref in $closed; do
        if ! grep -Eq "^#### ${ref}([[:space:]]|$)" done.md; then
            echo "leftovers: ${id} cites ${ref} as closing it, but done.md has no such card" >&2
            fail=1
        fi
    done
    for ref in $open; do
        if ! grep -Eq "^#### ${ref}([[:space:]]|$)" plan.md; then
            echo "leftovers: ${id} cites ${ref} as its §3 card, but plan.md has no such card" >&2
            fail=1
        fi
    done
    if [ "$stay" = "yes" ] && ! printf '%s\n' "$compat_ids" | grep -qx "$id"; then
        echo "leftovers: ${id} (done.md:${line}) needs a row in docs/compat.md 'Known leftovers'" >&2
        fail=1
    fi
    echo "leftovers: ${id} (done.md:${line}): closed by [${closed:-—}] task [${open:-—}] stays [${stay}]"
done <<< "$items"

# Both drift directions: an extracted leftover without a mapping, and a
# mapping (or compat row) whose leftover is gone.
for id in $(printf '%s\n' "$items" | cut -d'|' -f1 | sort -u); do
    case " $(mapped_ids) " in
        *" $id "*) ;;
        *)
            echo "leftovers: ${id} has a 'Not done:' line but no mapping in this script" >&2
            fail=1
            ;;
    esac
done
for id in $(mapped_ids); do
    if ! printf '%s\n' "$items" | cut -d'|' -f1 | grep -qx "$id"; then
        echo "leftovers: mapping for ${id} has no 'Not done:' line left in done.md" >&2
        fail=1
    fi
done
for id in $compat_ids; do
    if ! printf '%s\n' "$items" | cut -d'|' -f1 | grep -qx "$id"; then
        echo "leftovers: docs/compat.md rows ${id} with no 'Not done:' line left in done.md" >&2
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "leftovers: ok: ${count} items across $(printf '%s\n' "$items" | cut -d'|' -f1 | sort -u | wc -l | tr -d ' ') tasks, every one closed, tasked, or recorded"
