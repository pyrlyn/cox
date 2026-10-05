// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ChecklistRow` (DS§6.3 row `ChecklistRow`, the mockup's onboarding `.group .gr`): one step of
// the first-run checklist (DT§5.8) — a doctor check or a step still to take — with the one line
// that says what was found or what is missing, and the button that fixes it. Separate so every
// check says how it went by one symbol and one colour.

import SwiftUI

/// A status symbol in its colour, a `SettingLabel`, then the fix button at its natural size.
/// The detail stays in `text.secondary`: the colour is on the symbol, so the words stay
/// readable on frosted glass (DS§8).
public struct ChecklistRow: View {
  /// `cox doctor`'s ok, warn and fail, and a step the person has still to take.
  public enum Status: CaseIterable, Sendable {
    case passed, warning, missing, step
  }

  let title: String
  let detail: String
  let status: Status
  /// A DS§3.7 symbol for the step, or the status's own.
  let symbol: String
  /// The fix button's title, or `nil` for a row with nothing to do.
  let action: String?
  let perform: () -> Void

  init(
    _ title: String, detail: String, status: Status, symbol: String? = nil,
    action: String? = nil, perform: @escaping () -> Void = {}
  ) {
    self.title = title
    self.detail = detail
    self.status = status
    self.symbol = symbol ?? status.symbol
    self.action = action
    self.perform = perform
  }

  public var body: some View {
    HStack(spacing: Space.l) {
      Image(systemName: symbol)
        .symbolStyle(.body)
        .foregroundStyle(status.tint)
        // One column for every symbol, so the titles line up down the checklist.
        .frame(width: Size.iconTile)
        .accessibilityLabel(status.name)
      SettingLabel(title, detail: detail, namesItem: true)
        .frame(maxWidth: .infinity, alignment: .leading)
      if let action {
        Button(action, action: perform)
          .buttonStyle(CoxButtonStyle(status == .step ? .primary : .secondary, size: .small))
      }
    }
    // `SettingRow`'s insets, so the checklist reads as one more Settings group.
    .padding(.horizontal, Space.l)
    .padding(.vertical, Space.ml)
  }
}

extension ChecklistRow.Status {
  var symbol: String {
    switch self {
    case .passed: "checkmark"
    case .warning: "exclamationmark.triangle"
    case .missing: "xmark.octagon"
    case .step: "arrow.right"
    }
  }

  var tint: Color {
    switch self {
    case .passed: Color(.statusSuccess)
    case .warning: Color(.statusWarning)
    case .missing: Color(.statusDanger)
    case .step: Color(.accent)
    }
  }

  var name: String {
    switch self {
    case .passed: "OK"
    case .warning: "Warning"
    case .missing: "Missing"
    case .step: "To do"
    }
  }
}

#Preview("statuses") {
  PreviewMatrix {
    SettingsGroupBox(PreviewState.checksTitle) {
      ForEach(ChecklistRow.Status.allCases, id: \.self) { ChecklistRowSample(status: $0) }
    }
  }
}
