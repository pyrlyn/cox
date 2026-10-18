// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// File › Connect to Target…'s sheet (T52.21, T68.5): the target goes to CoxModel's `RemoteHosts`,
// which connects, saves an ssh alias to `desktop.remote_hosts` and keeps the groups; its `hello`
// answer comes back into the sheet before a session opens. Wiring only; separate from
// `SessionWindow` so the window's body stays the layout.

import CoxModel
import CoxUI
import SwiftUI

extension ConnectTargetSheet.State {
  /// The sheet as it opens: every kind, no targets yet (they load when it shows).
  static var opening: Self {
    Self(
      kinds: TargetKind.allCases.map {
        .init(id: $0.rawValue, title: $0.title, example: $0.example)
      })
  }
}

extension View {
  /// Shows the sheet while `connecting` is set; Connect keeps it up until the target answered and
  /// shows its `hello`, or says why it did not.
  func connectTargetSheet(
    _ connecting: Binding<ConnectTargetSheet.State?>, remotes: RemoteHosts
  ) -> some View {
    func connect(_ target: String) {
      Task { @MainActor in
        connecting.wrappedValue?.isConnecting = true
        connecting.wrappedValue?.failure = nil
        let failure = await remotes.connect(target)
        connecting.wrappedValue?.isConnecting = false
        connecting.wrappedValue?.failure = failure
        guard failure == nil else { return }
        // A connect with a connector always has a hello; without one there is nothing to show.
        guard let hello = remotes.hello(for: target) else {
          connecting.wrappedValue = nil
          return
        }
        connecting.wrappedValue?.hello = .init(summary: hello.summary, sandbox: hello.sandbox)
      }
    }
    return sheet(
      isPresented: Binding(
        get: { connecting.wrappedValue != nil },
        set: { if !$0 { connecting.wrappedValue = nil } })
    ) {
      ConnectTargetSheet(state: connecting.wrappedValue ?? .opening) { intent in
        switch intent {
        case .close: connecting.wrappedValue = nil
        case .connect(let target): connect(target)
        case .connectTyped(let kind, let name):
          if let kind = TargetKind(rawValue: kind),
            let text = TargetList.text(kind: kind, name: name)
          {
            connect(text)
          } else {
            connecting.wrappedValue?.failure = "A name has no spaces."
          }
        }
      }
      .task {
        let options = await remotes.targets().map {
          ConnectTargetSheet.Option(
            id: $0.target, kind: $0.kind.rawValue, name: $0.name, isConfigured: $0.isConfigured)
        }
        connecting.wrappedValue?.options = options
      }
    }
  }
}
