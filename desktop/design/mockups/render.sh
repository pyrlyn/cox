#!/bin/zsh
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Renders each mockup screen to a 2x PNG with headless Chrome; Chrome is killed once the file appears.
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
DIR=${0:A:h}
OUT=$DIR/screens
mkdir -p $OUT
for s in "$@"; do
  rm -f $OUT/$s.png
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
    --window-size=1520,980 --virtual-time-budget=1500 --no-first-run \
    --user-data-dir=$DIR/.chrome-$s --screenshot=$OUT/$s.png "file://$DIR/mockups.html#$s" >/dev/null 2>&1 &
  pid=$!
  for i in {1..60}; do [[ -s $OUT/$s.png ]] && break; sleep 0.5; done
  sleep 0.5; kill $pid 2>/dev/null; pkill -f "chrome-$s" 2>/dev/null
  rm -rf $DIR/.chrome-$s
  [[ -s $OUT/$s.png ]] && echo "ok $s" || echo "FAIL $s"
done
