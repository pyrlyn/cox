// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Spotlight index without Spotlight (T51.16): a row becomes an item with the title, the
// project and the time and no other text; a session that leaves the list is deleted; the first
// sync of a launch replaces what an earlier one left; a result's activity opens its session.

import CoreSpotlight
import Foundation
import Testing

@testable import CoxPlatform

private final class RecordingStore: SpotlightStore {
  var replaced: [[SpotlightRow]] = []
  var indexed: [[SpotlightRow]] = []
  var deleted: [[String]] = []

  func replace(_ rows: [SpotlightRow]) { replaced.append(rows) }
  func index(_ rows: [SpotlightRow]) { indexed.append(rows) }
  func delete(_ sessions: [String]) { deleted.append(sessions) }
}

private let written = Date(timeIntervalSince1970: 1_790_000_000)

private func row(_ session: String, _ title: String = "Fix the parser") -> SpotlightRow {
  SpotlightRow(session: session, title: title, project: "cox", lastActivity: written)
}

@Test func aRowBecomesAnItemWithTitleProjectAndTime() {
  let item = row("s1").item
  #expect(item.uniqueIdentifier == "s1")
  #expect(item.domainIdentifier == SpotlightRow.domain)
  #expect(item.attributeSet.title == "Fix the parser")
  #expect(item.attributeSet.contentDescription == "cox")
  #expect(item.attributeSet.contentModificationDate == written)
}

@Test func anItemCarriesNoTranscriptText() {
  let attributes = row("s1").item.attributeSet
  #expect(attributes.textContent == nil)
  #expect(attributes.htmlContentData == nil)
  let texts =
    [attributes.title, attributes.displayName, attributes.contentDescription]
    .compactMap { $0 } + (attributes.keywords ?? [])
  #expect(Set(texts) == ["Fix the parser", "cox"])
}

@MainActor
@Test func theFirstSyncReplacesWhatAnEarlierLaunchLeft() {
  let store = RecordingStore()
  SpotlightIndex(store: store).sync([row("s1"), row("s2", "")])
  #expect(store.replaced == [[row("s1")]])
  #expect(store.indexed.isEmpty && store.deleted.isEmpty)
}

@MainActor
@Test func anArchivedSessionIsDeletedOnTheNextSync() {
  let store = RecordingStore()
  let index = SpotlightIndex(store: store)
  index.sync([row("s1"), row("s2")])
  index.sync([row("s2")])
  #expect(store.deleted == [["s1"]])
  #expect(store.indexed.isEmpty)
}

@MainActor
@Test func onlyAChangedTitleIsWrittenAgain() {
  let store = RecordingStore()
  let index = SpotlightIndex(store: store)
  index.sync([row("s1"), row("s2")])
  index.sync([row("s1", "Fix the lexer"), row("s2")])
  #expect(store.indexed == [[row("s1", "Fix the lexer")]])
  #expect(store.deleted.isEmpty)
}

@Test func aSpotlightResultOpensItsSession() {
  let activity = NSUserActivity(activityType: CSSearchableItemActionType)
  activity.userInfo = [CSSearchableItemActivityIdentifier: "s1"]
  #expect(SpotlightIndex.session(from: activity) == "s1")
  #expect(SpotlightIndex.session(from: NSUserActivity(activityType: "other")) == nil)
}
