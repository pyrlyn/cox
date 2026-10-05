// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ConnectHostSheet` (DS§6.4, DT§4.4, T52.21): File › Connect to Host…, one `ssh` host alias
// and Connect. The host runs `cox app-server --stdio` over the person's own ssh; the sheet says
// that no key and no ssh agent leave this Mac. Separate so the app only presents it: the core
// checks the alias, dials and reports why a connect failed, and the sheet shows that line.

import SwiftUI

/// The alias field over a caption, the last failure in `status.danger`, then Cancel and Connect.
public struct ConnectHostSheet: View {
  public struct State: Equatable, Sendable {
    /// Why the last connect failed, as the core said it.
    public var failure: String?
    /// A connect runs; Connect waits.
    public var isConnecting: Bool

    public init(failure: String? = nil, isConnecting: Bool = false) {
      (self.failure, self.isConnecting) = (failure, isConnecting)
    }
  }

  public enum Intent: Equatable, Sendable {
    case connect(String)
    case cancel
  }

  let state: State
  let send: (Intent) -> Void
  @SwiftUI.State private var draft: String

  public init(state: State, host: String = "", send: @escaping (Intent) -> Void) {
    (self.state, self.send) = (state, send)
    _draft = SwiftUI.State(initialValue: host)
  }

  private var alias: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.l) {
      Text("Connect to Host").textStyle(.body).fontWeight(.semibold)
        .foregroundStyle(Color(.textPrimary))
        .accessibilityAddTraits(.isHeader)
      Text(
        "An ssh host alias. cox runs there over your ssh; no key or ssh agent leaves this Mac."
      )
      .textStyle(.caption)
      .foregroundStyle(Color(.textSecondary))
      .fixedSize(horizontal: false, vertical: true)
      HStack(spacing: Space.s) {
        Image(systemName: "server.rack")
          .symbolStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
        TextField(
          "Host", text: $draft, prompt: Text("devbox").foregroundStyle(Color(.textPlaceholder))
        )
        .textFieldStyle(.plain)
        .textStyle(.monoInline)
        .foregroundStyle(Color(.textPrimary))
        .onSubmit(connect)
      }
      .padding(.horizontal, Space.ml)
      .frame(height: Size.buttonHeight)
      .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
      if let failure = state.failure {
        Text(failure).textStyle(.caption).foregroundStyle(Color(.statusDanger))
          .fixedSize(horizontal: false, vertical: true)
      }
      HStack(spacing: Space.s) {
        Spacer(minLength: 0)
        Button("Cancel") { send(.cancel) }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          .keyboardShortcut(.cancelAction)
        Button(state.isConnecting ? "Connecting…" : "Connect", action: connect)
          .buttonStyle(CoxButtonStyle(.primary, size: .small))
          .keyboardShortcut(.defaultAction)
          .disabled(alias.isEmpty || state.isConnecting)
      }
    }
    .padding(Space.xl)
    .frame(width: Size.popoverWidth)
  }

  private func connect() {
    guard !alias.isEmpty, !state.isConnecting else { return }
    send(.connect(alias))
  }
}

#Preview("empty") { PreviewMatrix { ConnectHostSheet(state: .init()) { _ in } } }
#Preview("failed") {
  PreviewMatrix {
    ConnectHostSheet(state: PreviewState.connectHostFailed, host: "devbox") { _ in }
  }
}
