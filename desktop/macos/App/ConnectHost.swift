// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// File › Connect to Host…'s sheet (T52.21): the alias goes to CoxModel's `RemoteHosts`, which
// connects, saves it to `desktop.remote_hosts` and keeps the host groups. Wiring only; separate
// from `SessionWindow` so the window's body stays the layout.

import CoxModel
import CoxUI
import SwiftUI

extension View {
  /// Shows the sheet while `connecting` is set; Connect keeps it up until the host answered and
  /// says why when it did not.
  func connectHostSheet(
    _ connecting: Binding<ConnectHostSheet.State?>, remotes: RemoteHosts
  ) -> some View {
    sheet(
      isPresented: Binding(
        get: { connecting.wrappedValue != nil },
        set: { if !$0 { connecting.wrappedValue = nil } })
    ) {
      ConnectHostSheet(state: connecting.wrappedValue ?? .init()) { intent in
        switch intent {
        case .cancel: connecting.wrappedValue = nil
        case .connect(let host):
          Task { @MainActor in
            connecting.wrappedValue = ConnectHostSheet.State(isConnecting: true)
            let failure = await remotes.connect(host)
            connecting.wrappedValue = failure.map { ConnectHostSheet.State(failure: $0) }
          }
        }
      }
    }
  }
}
