// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ModeSegmented` (DS§6.3 row `ModeSegmented`, the mockup's toolbar `.seg`): the session's
// permission mode — Ask, Plan, Auto, Bypass — each marked in its DS§3.1 colour. Separate so
// Settings' default mode picks a mode with the same control and colours; `ModeMenu`, here beside
// it, is the same choice as the composer's mode chip (T60.6, DT§5.3).

import SwiftUI

/// The session's permission modes, in the order the toolbar shows them: the core's
/// `PermissionMode`, with `default` named Ask as the person sees it. Public because the composer's
/// mode chip (T37.24.7) shows the same mode with the same title and colour.
public enum SessionMode: CaseIterable, Sendable {
  case ask, plan, auto, bypass
}

/// A `CoxSegmented` over the modes. Bypass is offered only while it is on: it is turned on
/// elsewhere (a confirmation, a setting), never by a stray click beside Auto.
struct ModeSegmented: View {
  typealias Mode = SessionMode

  @Binding var selection: Mode

  init(selection: Binding<Mode>) {
    self._selection = selection
  }

  var body: some View {
    CoxSegmented(
      "Mode", selection: $selection, options: Mode.offered(selection),
      look: \.look, title: { Text($0.title) })
  }
}

extension ModeSegmented.Mode {
  /// The modes the control offers while `selection` is on.
  static func offered(_ selection: Self) -> [Self] {
    selection == .bypass ? allCases : [.ask, .plan, .auto]
  }

  var title: String {
    switch self {
    case .ask: "Ask"
    case .plan: "Plan"
    case .auto: "Auto"
    case .bypass: "Bypass"
    }
  }

  /// DS§3.1: Ask in `text.primary`, Plan in `status.plan`, Auto in `accent`, Bypass filled
  /// with `status.danger`.
  var look: SegmentLook {
    switch self {
    case .ask: .plain
    case .plan: .tinted(Color(.statusPlan))
    case .auto: .tinted(Color(.accent))
    case .bypass: .filled(Color(.statusDanger))
    }
  }
}

/// The composer's mode chip as a menu of all four modes, the one in force checked. Choosing Bypass
/// asks first: it was never one click away on the old segmented control (it was offered only
/// while on), and it lets tools run without asking.
struct ModeMenu: View {
  /// What choosing a mode does.
  enum Request: Equatable {
    case none, pick, confirm
  }

  let mode: SessionMode
  let pick: (SessionMode) -> Void
  @State private var isConfirmingBypass = false

  var body: some View {
    Menu {
      Picker("Mode", selection: Binding(get: { mode }, set: choose)) {
        ForEach(SessionMode.allCases, id: \.self) { Text($0.title) }
      }
      .pickerStyle(.inline)
    } label: {
      ComposerChip(mode.title, kind: .mode(mode), shortcut: "⇧⇥")
    }
    .menuStyle(.button)
    .menuIndicator(.hidden)
    .buttonStyle(.plain)
    .fixedSize()
    .help("Permission mode (⇧⇥ for the next)")
    .accessibilityLabel("Permission mode")
    .accessibilityValue(mode.title)
    .confirmationDialog(
      "Turn on Bypass mode?", isPresented: $isConfirmingBypass, titleVisibility: .visible
    ) {
      Button("Turn on Bypass", role: .destructive) { pick(.bypass) }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text("Tools run without asking. The shell still runs inside the sandbox.")
    }
  }

  private func choose(_ next: SessionMode) {
    switch Self.request(next, from: mode) {
    case .none: break
    case .pick: pick(next)
    case .confirm: isConfirmingBypass = true
    }
  }

  /// A mode already in force asks for nothing; Bypass asks for a confirmation first.
  static func request(_ next: SessionMode, from current: SessionMode) -> Request {
    if next == current { return .none }
    return next == .bypass ? .confirm : .pick
  }
}

#Preview("ask") { PreviewMatrix { ModeSegmented(selection: .constant(.ask)) } }
#Preview("plan") { PreviewMatrix { ModeSegmented(selection: .constant(.plan)) } }
#Preview("auto") { PreviewMatrix { ModeSegmented(selection: .constant(.auto)) } }
#Preview("bypass") { PreviewMatrix { ModeSegmented(selection: .constant(.bypass)) } }
