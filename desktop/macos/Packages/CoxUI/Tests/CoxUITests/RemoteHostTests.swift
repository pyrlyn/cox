// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.21's CoxUI check (DS§6.4, DT§4.4): a remote host's sidebar group, connected and
// disconnected, in light/dark × Solid/Frosted. The Connect to Target sheet is in
// `ConnectTargetSheetTests`.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct RemoteHostSnapshotTests {
  @Test(arguments: Variant.all) func hostGroup(_ variant: Variant) throws {
    try assertCoxSnapshot(
      Sidebar(state: PreviewState.hostSidebar) { _ in }.frame(height: Size.windowMinHeight),
      variant, named: variant.name)
  }
}

@Suite struct RemoteHostTests {
  @Test func aDisconnectedHostsRowsAreReadOnly() {
    let hosts = PreviewState.hostSidebar.groups.filter {
      if case .host = $0.kind { true } else { false }
    }
    #expect(hosts.map(\.kind) == [.host(isConnected: true), .host(isConnected: false)])
    #expect(hosts[0].sessions.allSatisfy { !$0.isReadOnly })
    #expect(hosts[1].sessions.allSatisfy { $0.isReadOnly })
  }
}
