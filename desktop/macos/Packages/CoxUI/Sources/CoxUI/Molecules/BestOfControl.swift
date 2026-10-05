// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `BestOfControl` (DT§3.3.1, mockup 27's "Compare with a second agent on the same prompt"): the
// composer's best-of-n control. The person adds candidates beside the session's own agent, then
// "Best of n" sends the composer's prompt to all of them, each in a worktree of its own. Separate
// so the app only presents it: CoxModel's `BestOfStore` and the core own the launch.

import SwiftUI

/// A caption, the "Best of n" capsule that launches, then one capsule per candidate that can be
/// added — "+ label", or a checkmark once added — and the last failure in `status.danger`.
public struct BestOfControl: View {
  public struct Option: Equatable, Sendable, Identifiable {
    /// What `Intent.toggle` names.
    public var id: String
    public var label: String
    public var isPicked: Bool
    /// Why it cannot start here; it cannot be added then.
    public var unavailable: String?

    public init(id: String, label: String, isPicked: Bool = false, unavailable: String? = nil) {
      (self.id, self.label, self.isPicked, self.unavailable) = (id, label, isPicked, unavailable)
    }
  }

  public struct State: Equatable, Sendable {
    public var options: [Option]
    public var isLaunching: Bool
    /// Why the last launch failed.
    public var failure: String?

    public init(options: [Option] = [], isLaunching: Bool = false, failure: String? = nil) {
      (self.options, self.isLaunching, self.failure) = (options, isLaunching, failure)
    }

    /// The session's own agent is one of the n.
    public var count: Int { 1 + options.filter(\.isPicked).count }
    public var canLaunch: Bool { count > 1 && !isLaunching }
  }

  public enum Intent: Equatable, Sendable {
    case toggle(String)
    case launch
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.m) {
      Text("Compare with a second agent on the same prompt")
        .textStyle(.caption)
        .foregroundStyle(Color(.textSecondary))
      ScrollView(.horizontal) {
        HStack(spacing: Space.m) {
          Button {
            send(.launch)
          } label: {
            Label("Best of \(state.count)", systemImage: "person.2")
          }
          .buttonStyle(CapsuleStyle(state.canLaunch ? .active : .plain))
          .disabled(!state.canLaunch)
          ForEach(state.options) { option in
            Button {
              send(.toggle(option.id))
            } label: {
              if option.isPicked {
                Label(option.label, systemImage: "checkmark")
              } else {
                Text(verbatim: "+ \(option.label)")
              }
            }
            .buttonStyle(CapsuleStyle(option.isPicked ? .active : .plain))
            .disabled(option.unavailable != nil || state.isLaunching)
            .help(option.unavailable ?? "")
            .accessibilityAddTraits(option.isPicked ? .isSelected : [])
          }
        }
      }
      .scrollIndicators(.never)
      if let failure = state.failure {
        Text(failure).textStyle(.caption).foregroundStyle(Color(.statusDanger))
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .padding(Space.l)
    .frame(maxWidth: .infinity, alignment: .leading)
    .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
  }
}

#Preview("best of") {
  PreviewMatrix { BestOfControl(state: PreviewState.bestOfControl) { _ in } }
}
