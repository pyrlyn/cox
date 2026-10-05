// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The sidebar's "Needs you" rows (T37.27.7, DS§6.4 `Sidebar`): one row per inbox item, two of
// one session among them, per light/dark × Solid/Frosted cell; an item's row opens and
// highlights its session.

import Testing

@testable import CoxUI

@MainActor
@Suite struct SidebarInboxSnapshotTests {
  @Test(arguments: Variant.all) func needsYouSidebar(_ variant: Variant) throws {
    try assertCoxSnapshot(
      Sidebar(state: PreviewState.inboxSidebar) { _ in }.frame(height: Size.windowMinHeight),
      variant, named: variant.name)
  }

  /// An expired item's row keeps its text in `text.secondary` instead of the faded label of a
  /// disabled `.plain` button, which fell below DS§8; short, so the row's text is a large share.
  @Test(arguments: [Variant.all[0], Variant.all[2]]) func expiredItemRowStaysReadable(
    _ variant: Variant
  ) throws {
    var state = PreviewState.inboxSidebar
    state.groups[0].sessions.removeAll { !$0.isReadOnly }
    try assertCoxSnapshot(
      Sidebar(state: state) { _ in }.frame(height: Size.windowMinHeight / 2), variant,
      named: variant.name)
  }
}

@Suite struct SidebarInboxTests {
  @Test func anItemRowOpensItsSessionAndAPlainRowItself() {
    let rows = PreviewState.inboxSidebar.groups[0].sessions
    #expect(rows.map(\.opens) == ["pkce", "pkce", "seo", "flaky"])
    #expect(rows.map(\.isReadOnly) == [false, false, false, true])
    #expect(PreviewState.sidebar.groups[0].sessions.map(\.opens) == ["pkce", "flaky"])
  }
}
