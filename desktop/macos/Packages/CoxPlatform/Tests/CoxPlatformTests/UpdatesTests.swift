// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The updater's two decisions (T37.32.2): whether this build checks at all, read from its
// Info.plist, and whether a relaunch for an update waits for a yes. No updater starts here.

import Foundation
import Testing

@testable import CoxPlatform

private let feed = "https://github.com/pyrlyn/cox/releases/latest/download/appcast.xml"

@Test func aBuildWithAnHttpsFeedAndAKeyChecksForUpdates() {
  let info: [String: Any] = ["SUFeedURL": feed, "SUPublicEDKey": "a-base64-ed25519-key"]
  #expect(UpdateFeed.configured(info)?.url == URL(string: feed))
}

@Test func aBuildWithoutAKeyOrWithAPlainFeedNeverChecks() {
  #expect(UpdateFeed.configured(["SUFeedURL": feed, "SUPublicEDKey": ""]) == nil)
  #expect(UpdateFeed.configured(["SUFeedURL": feed]) == nil)
  #expect(
    UpdateFeed.configured(["SUFeedURL": "http://example.com/a.xml", "SUPublicEDKey": "k"]) == nil)
  #expect(UpdateFeed.configured(["SUFeedURL": "", "SUPublicEDKey": "k"]) == nil)
}

@MainActor
@Test func withNoSessionRunningTheRelaunchGoesAheadWithoutAsking() {
  var asked = false
  let postponed = AppUpdater.postpones(
    running: 0,
    confirm: { _ in
      asked = true
      return false
    }, install: {})
  #expect(!postponed)
  #expect(!asked)
}

@MainActor
@Test func aRunningSessionPostponesTheRelaunchUntilTheUserSaysYes() async {
  let installs = Counter()
  let postponed = AppUpdater.postpones(
    running: 2, confirm: { $0 == 2 }, install: { installs.bump() })
  #expect(postponed)
  await Task.yield()
  #expect(installs.count == 1)
}

@MainActor
@Test func noLeavesTheUpdateForQuit() async {
  let installs = Counter()
  let postponed = AppUpdater.postpones(
    running: 1, confirm: { _ in false }, install: { installs.bump() })
  #expect(postponed)
  await Task.yield()
  #expect(installs.count == 0)
}

@MainActor
private final class Counter {
  var count = 0
  func bump() { count += 1 }
}
