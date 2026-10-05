// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `RiskChip` (DS§6.2 row `RiskChip`, the mockup's `.risk`): what a tool call may do — "network
// · writes remote" — tinted by how risky the classifier found it. Separate so a risk reads the
// same on a tool row and an approval card; it draws as a `Badge`, the mockup's `.risk` and
// `.badge` being the same shape.

import SwiftUI

/// The reason text as a badge in its level's `risk.*` role: low is quiet, medium warns, high is
/// danger.
public struct RiskChip: View {
  public enum Level: CaseIterable, Sendable {
    case low, medium, high
  }

  let text: String
  let level: Level

  init(_ text: String, level: Level) {
    self.text = text
    self.level = level
  }

  public var body: some View {
    Badge(text, foreground: level.foreground, background: level.background)
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("\(level.label): \(text)")
  }
}

extension RiskChip.Level {
  var foreground: Color {
    switch self {
    case .low: Color(.riskLow)
    case .medium: Color(.riskMedium)
    case .high: Color(.riskHigh)
    }
  }

  var background: Color {
    switch self {
    case .low: Color(.riskLowSoft)
    case .medium: Color(.riskMediumSoft)
    case .high: Color(.riskHighSoft)
    }
  }

  var label: String {
    switch self {
    case .low: "Low risk"
    case .medium: "Medium risk"
    case .high: "High risk"
    }
  }
}

#Preview("low") { PreviewMatrix { RiskChip(PreviewState.risk, level: .low) } }
#Preview("medium") { PreviewMatrix { RiskChip(PreviewState.risk, level: .medium) } }
#Preview("high") { PreviewMatrix { RiskChip(PreviewState.risk, level: .high) } }
