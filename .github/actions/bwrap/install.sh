#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Linux job A (plan.md T4.2): install bubblewrap and let it create user
# namespaces, so the sandbox picks bwrap and can wrap an argv, then export
# COX_EXPECT_SANDBOX=bwrap. Run by .github/actions/bwrap and by the `rust`
# job's setup-command in ci.yml (pyrlyn/infra ci-rust.yml).
set -euo pipefail
sudo apt-get update
sudo apt-get install -y bubblewrap
# Ubuntu 24.04 confines unprivileged user namespaces behind AppArmor
# and ships no profile for bwrap, so `bwrap --unshare-user` fails and
# `sandbox::backend` silently falls back to Landlock, which cannot
# wrap an argv. Lift the restriction here, then run the exact probe
# cox runs so a refusal fails this step with bwrap's own message
# rather than a test minutes later.
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
bwrap --unshare-user --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev /bin/true
echo "COX_EXPECT_SANDBOX=bwrap" >> "$GITHUB_ENV"
