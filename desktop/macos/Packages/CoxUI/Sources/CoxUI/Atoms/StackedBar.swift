// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `StackedBar(segments)` (DS§6.2 row `StackedBar(segments)`, the mockup's `.tokpop .bar`): how
// the context window is split between its parts (DS§7), in the token popover and the context
// inspector. Separate so each part has one colour wherever the split is drawn, taken from the
// `context.*` tokens rather than from the caller.

import SwiftUI

/// Segments laid end to end from the leading edge over a `fill.secondary` capsule; what they
/// leave uncovered is the free share.
public struct StackedBar: View {
  /// A part of the context window (DS§7); the raw value is the core's `ContextPart.kind`.
  public enum Kind: String, CaseIterable, Sendable {
    case system, tools, instructions, history
  }

  struct Segment: Equatable, Sendable {
    let kind: Kind
    /// The part's share of the whole bar.
    let fraction: Double
  }

  /// The segments as drawn: none negative, and none past the end of the bar.
  let segments: [Segment]

  @Environment(\.locale) private var locale

  /// The mockup's `.bar`, `height: 12px`: no size token is a bar.
  private static let height: CGFloat = 12

  init(_ segments: [Segment]) {
    var used = 0.0
    self.segments = segments.map { segment in
      let share = segment.fraction.isNaN ? 0 : min(max(segment.fraction, 0), max(1 - used, 0))
      used += share
      return Segment(kind: segment.kind, fraction: share)
    }
  }

  public var body: some View {
    GeometryReader { proxy in
      HStack(spacing: 0) {
        ForEach(segments.indices, id: \.self) { index in
          Rectangle()
            .fill(segments[index].kind.colour)
            .frame(width: proxy.size.width * segments[index].fraction)
        }
      }
    }
    .frame(height: Self.height)
    .background(Color(.fillSecondary))
    .clipShape(Capsule())
    .accessibilityElement()
    .accessibilityLabel(label(locale))
  }

  /// Each part and its share, in the bar's order: "system 3%, history 24%".
  func label(_ locale: Locale) -> String {
    segments.map { segment in
      let share = segment.fraction.formatted(
        .percent.precision(.fractionLength(0)).locale(locale))
      return "\(segment.kind.name) \(share)"
    }
    .joined(separator: ", ")
  }
}

extension StackedBar.Kind {
  var colour: Color {
    switch self {
    case .system: Color(.contextSystem)
    case .tools: Color(.contextTools)
    case .instructions: Color(.contextInstructions)
    case .history: Color(.contextHistory)
    }
  }

  /// The part as DS§7 names it.
  var name: String {
    switch self {
    case .system: "system"
    case .tools: "tools"
    case .instructions: "instruction files"
    case .history: "history"
    }
  }
}

#Preview("empty") { PreviewMatrix { StackedBarSample(PreviewState.bars[0].segments) } }
#Preview("history") { PreviewMatrix { StackedBarSample(PreviewState.bars[1].segments) } }
#Preview("turn") { PreviewMatrix { StackedBarSample(PreviewState.bars[2].segments) } }
#Preview("full") { PreviewMatrix { StackedBarSample(PreviewState.bars[3].segments) } }
