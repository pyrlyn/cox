// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Session titles in Spotlight (T51.16): each listed session's title, its project and its last
// write, and nothing else; never transcript text. Titles arrive already sanitized by Rust. A
// session that leaves the workspace's list (archived, deleted or aged out of it) leaves the
// index on the next sync. Opening a result continues as `CSSearchableItemActionType`, and
// `session(from:)` reads the session back from it. The index sits behind `SpotlightStore`, so
// the mapping and the diff are tested without Spotlight.

import CoreSpotlight
import Foundation
import UniformTypeIdentifiers

/// One session as Spotlight shows it.
public struct SpotlightRow: Equatable, Sendable {
  public let session: String
  public let title: String
  /// The project's name.
  public let project: String
  public let lastActivity: Date?

  public init(session: String, title: String, project: String, lastActivity: Date?) {
    (self.session, self.title, self.project, self.lastActivity) = (
      session, title, project, lastActivity
    )
  }

  /// Cox's domain in the index; a launch clears it before indexing anew.
  public static let domain = "sessions"

  /// The searchable item: title, project and time only.
  public var item: CSSearchableItem {
    let attributes = CSSearchableItemAttributeSet(contentType: .content)
    attributes.title = title
    attributes.displayName = title
    attributes.contentDescription = project
    attributes.keywords = [project]
    attributes.contentModificationDate = lastActivity
    return CSSearchableItem(
      uniqueIdentifier: session, domainIdentifier: Self.domain, attributeSet: attributes)
  }
}

/// Where the rows go: Spotlight in the app, a recorder in a test.
public protocol SpotlightStore {
  /// Everything in cox's domain becomes `rows`.
  func replace(_ rows: [SpotlightRow])
  /// Adds or updates `rows`.
  func index(_ rows: [SpotlightRow])
  func delete(_ sessions: [String])
}

/// The system's on-device index. A failed write is dropped: the next sync writes again.
public struct CoreSpotlightStore: SpotlightStore {
  public init() {}

  public func replace(_ rows: [SpotlightRow]) {
    // In the deletion's completion, so the old domain cannot outlive the new rows.
    let index = CSSearchableIndex.default()
    index.deleteSearchableItems(withDomainIdentifiers: [SpotlightRow.domain]) { _ in
      CSSearchableIndex.default().indexSearchableItems(rows.map(\.item))
    }
  }

  public func index(_ rows: [SpotlightRow]) {
    CSSearchableIndex.default().indexSearchableItems(rows.map(\.item))
  }

  public func delete(_ sessions: [String]) {
    CSSearchableIndex.default().deleteSearchableItems(withIdentifiers: sessions)
  }
}

@MainActor
public final class SpotlightIndex {
  private let store: any SpotlightStore
  /// What the store holds, by session; `nil` before the first sync of this launch.
  private var indexed: [String: SpotlightRow]?

  public init(store: any SpotlightStore) { self.store = store }

  /// Makes the index hold `rows`: the first sync replaces what an earlier launch left, each
  /// later one writes only what changed and deletes what left the list. An untitled session has
  /// nothing to find and is left out.
  public func sync(_ rows: [SpotlightRow]) {
    let titled = rows.filter { !$0.title.isEmpty }
    let now = Dictionary(titled.map { ($0.session, $0) }, uniquingKeysWith: { first, _ in first })
    defer { self.indexed = now }
    guard let indexed else { return store.replace(titled) }
    let gone = indexed.keys.filter { now[$0] == nil }.sorted()
    let changed = titled.filter { indexed[$0.session] != $0 }
    if !gone.isEmpty { store.delete(gone) }
    if !changed.isEmpty { store.index(changed) }
  }

  /// The session a Spotlight result opens; `nil` for any other activity.
  public nonisolated static func session(from activity: NSUserActivity) -> String? {
    guard activity.activityType == CSSearchableItemActionType else { return nil }
    return activity.userInfo?[CSSearchableItemActivityIdentifier] as? String
  }
}
