// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SessionRow` (DS§6.3 row `SessionRow`, the mockup's `.row`): one session in the sidebar — its
// state, title, what it is doing and what it has cost. Separate so every list of sessions
// (sidebar groups, search results) draws a session the same way.

import SwiftUI

/// A `StatusDot` centred on the title line, the title over a subtitle, the cost at the far
/// edge; selected, it takes `InspectorRow`'s `rowSelection` on `accent.selected`, where both its
/// text colours hold DS§8's 4.5:1 in every material (DS§3.4, A115).
public struct SessionRow: View {
  /// What the row shows, formatted by the core.
  public struct Item: Equatable, Sendable {
    public var status: StatusDot.Status
    public var title: String
    /// Project and activity, `cox · running cargo nextest`.
    public var subtitle: String
    /// The session's cost, `$0.42`, or `nil` for none yet.
    public var cost: String?

    public init(status: StatusDot.Status, title: String, subtitle: String, cost: String? = nil) {
      (self.status, self.title, self.subtitle, self.cost) = (status, title, subtitle, cost)
    }
  }

  let item: Item
  let isSelected: Bool
  /// Off for a row that answers nothing (an expired inbox item): its title in `text.secondary`.
  @Environment(\.isEnabled) private var isEnabled

  init(_ item: Item, isSelected: Bool = false) {
    self.item = item
    self.isSelected = isSelected
  }

  public var body: some View {
    HStack(alignment: .titleLine, spacing: Space.m) {
      StatusDot(item.status)
      VStack(alignment: .leading, spacing: 0) {
        HStack(alignment: .firstTextBaseline, spacing: Space.m) {
          Text(item.title)
            .textStyle(.titleSession)
            .foregroundStyle(Color(isEnabled ? .textPrimary : .textSecondary))
            .frame(maxWidth: .infinity, alignment: .leading)
          if let cost = item.cost {
            // `text.secondary`, not the mockup's tertiary: a figure must stay readable on
            // frosted glass (DS§8).
            Text(cost)
              .textStyle(.detail, tabularDigits: true)
              .foregroundStyle(Color(.textSecondary))
          }
        }
        .alignmentGuide(.titleLine) { $0[VerticalAlignment.center] }
        Text(item.subtitle)
          .textStyle(.detail)
          .foregroundStyle(Color(.textSecondary))
      }
    }
    .lineLimit(1)
    .padding(.horizontal, Space.ml)
    .padding(.vertical, Space.s)
    .rowSelection(isSelected, fill: .accentSelected)
  }
}

extension VerticalAlignment {
  /// The middle of a row's title line, where its dot sits.
  private enum TitleLine: AlignmentID {
    static func defaultValue(in dimensions: ViewDimensions) -> CGFloat {
      dimensions[VerticalAlignment.center]
    }
  }

  fileprivate static let titleLine = VerticalAlignment(TitleLine.self)
}

#Preview("selected") {
  PreviewMatrix {
    SessionRow(PreviewState.sessions[0], isSelected: true).frame(width: Size.sidebarWidth)
  }
}
#Preview("list") { PreviewMatrix { SessionList() } }
