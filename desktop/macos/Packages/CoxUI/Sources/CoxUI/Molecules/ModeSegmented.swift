// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ModeSegmented` (DS§6.3 row `ModeSegmented`, the mockup's toolbar `.seg`): the session's
// permission mode — Ask, Plan, Auto, Bypass — each marked in its DS§3.1 colour. Separate so the
// toolbar and Settings' default mode pick a mode with the same control and colours.

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

#Preview("ask") { PreviewMatrix { ModeSegmented(selection: .constant(.ask)) } }
#Preview("plan") { PreviewMatrix { ModeSegmented(selection: .constant(.plan)) } }
#Preview("auto") { PreviewMatrix { ModeSegmented(selection: .constant(.auto)) } }
#Preview("bypass") { PreviewMatrix { ModeSegmented(selection: .constant(.bypass)) } }
