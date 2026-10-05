// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The sidebar's "Needs you" section (T37.27.7, DT§4.3 Inbox, DS§6.4 `Sidebar`): the app inbox as
// one row per item, an expired one read-only, in the core's words (T58.4.1). Here, not in CoxUI,
// because the store owns what the section shows (DS§1); the app copies each row into CoxUI's
// `Sidebar.Session` field for field and re-reads the inbox whenever the host hears of a new item
// or a lower badge.

import CoxClient
import Observation

public struct InboxRow: Identifiable, Equatable, Sendable {
  /// The sidebar's status glyph, named as CoxUI's `StatusDot.Status`.
  public typealias Status = InboxStatus

  /// The item's own id, as one session can wait on several.
  public let id: String
  /// The session a click opens.
  public let session: String
  public let status: Status
  /// The tool and its subject, the question, the error or the task label.
  public let title: String
  /// What it waits for, after the subagent that asked.
  public let subtitle: String
  /// The session closed or moved to another process: shown, not answerable from here.
  public let isReadOnly: Bool
}

extension InboxRow {
  public init(_ item: InboxItem) {
    id = "\(item.session)#\(item.seq)"
    session = item.session
    status = item.status
    title = item.title
    subtitle = item.subtitle
    isReadOnly = item.expired
  }
}

@Observable
@MainActor
public final class InboxStore {
  /// In the core's order: most urgent first, oldest first within a rank.
  public private(set) var rows: [InboxRow] = []
  /// The same items as the core sent them, for the menu bar's Allow and Deny (T51.14).
  public private(set) var items: [InboxItem] = []
  @ObservationIgnored private let client: any InboxClient

  public init(client: any InboxClient) { self.client = client }

  /// Reads the inbox again; the app calls it on each `PlatformHost.notify` and `badge`.
  public func refresh() {
    items = client.inbox()
    rows = items.map(InboxRow.init)
  }

  /// The section header's count, `nil` when nothing waits.
  public var count: String? { rows.isEmpty ? nil : "\(rows.count)" }
}
