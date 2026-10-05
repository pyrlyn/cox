// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ContextTab` (DS§6.4 row `ContextTab`, mockup screen 10's context block; DT§5.1 Context &
// Cost): the inspector's third tab — how the model's window is split between system, tools,
// instruction files and history, what is left, the turn's cache hit, and "Compact now". Separate
// so the `Inspector` frame stays a slot and each tab is its own view, fed plain values the app
// copies from the core's token meter (T37.29.3.1) and cost history (T37.29.3.2), the project's
// spend as the footnote (T37.29.3.3). The cache hit is the turn's or the session's, as the app
// picks (A104); "Compact now" waits while a turn runs (A105). The budget caps against the spend
// come from the same cost history (T37.29.3.4).

import SwiftUI

/// An `InspectorSection` headed by the context and its share of the window: the StackedBar, a
/// legend row per part and one for what is free, and "Compact now"; then the cache hit and the
/// cost by turn; the budget caps with a gauge each; the project's spend as a footnote. Before the core sent anything, one quiet line.
public struct ContextTab: View {
  /// What the tab shows, formatted by the core.
  public struct State: Equatable, Sendable {
    /// `Context · 76.4k` and `7.6% of 1M` (empty while the window is unknown).
    public var context = "", share = ""
    /// In the bar's order; none hides the section.
    public var parts: [Part] = []
    /// `923.6k`, the window less the parts; empty while the window is unknown.
    public var free = ""
    /// `94% this turn` or `88% this session`.
    public var cacheHit = ""
    /// A turn is running: compaction would rewrite the context it uses, so "Compact now" waits.
    public var turnRunning = false
    /// `In`, `Out`, `Cache r/w`, `$`: the cost grid's value columns.
    public var costColumns: [String] = []
    /// A row per turn, its subagents indented under it, the session total last; none hides
    /// the section.
    public var costs: [KeyValueGrid.Row] = []
    /// The session's and the month's spend against their caps; none hides the section.
    public var budget: [Budget] = []
    /// `Project cox today: $3.18 · this week: $21.40. …`, under everything; empty hides it.
    public var footnote = ""

    public init(
      context: String = "", share: String = "", parts: [Part] = [], free: String = "",
      cacheHit: String = "", turnRunning: Bool = false, costColumns: [String] = [],
      costs: [KeyValueGrid.Row] = [], budget: [Budget] = [], footnote: String = ""
    ) {
      (self.context, self.share, self.parts, self.free) = (context, share, parts, free)
      (self.cacheHit, self.turnRunning) = (cacheHit, turnRunning)
      (self.costColumns, self.costs, self.budget, self.footnote) =
        (costColumns, costs, budget, footnote)
    }
  }

  /// A cap against its spend: `Session`, `$0.42 of $5.00` and a gauge at its share.
  public struct Budget: Equatable, Sendable {
    public var label, text: String
    /// The spend over the cap, 0...1; nil without a usable cap, so no gauge.
    public var fraction: Double?

    public init(label: String, text: String, fraction: Double? = nil) {
      (self.label, self.text, self.fraction) = (label, text, fraction)
    }
  }

  /// A part of the window: its colour role and share of the bar, `System` and `7.6k`.
  public struct Part: Equatable, Sendable {
    public var kind: StackedBar.Kind
    public var fraction: Double
    public var label, tokens: String

    public init(kind: StackedBar.Kind, fraction: Double, label: String, tokens: String) {
      (self.kind, self.fraction, self.label, self.tokens) = (kind, fraction, label, tokens)
    }
  }

  /// What the tab asks the app to do.
  public enum Intent: Equatable, Sendable {
    /// Compact the conversation now, as `/compact` does.
    case compact
  }

  let state: State
  let send: @MainActor (Intent) -> Void

  /// The mockup's legend swatch, 8 pt, as the token popover draws it.
  private static let swatch: CGFloat = 8
  /// A cap's gauge, thinner than the context's 12 pt bar because it sits under a text line.
  private static let gaugeHeight: CGFloat = 6

  public init(state: State, send: @escaping @MainActor (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.xl) {
      if !state.parts.isEmpty {
        InspectorSection(state.context) {
          figure(state.share)
        } rows: {
          VStack(alignment: .leading, spacing: Space.l) {
            StackedBar(state.parts.map { .init(kind: $0.kind, fraction: $0.fraction) })
            legend
            Button("Compact now") { send(.compact) }
              .buttonStyle(CoxButtonStyle(.secondary, size: .small))
              .disabled(state.turnRunning)
          }
        }
      }
      if !state.cacheHit.isEmpty {
        SectionHeader("Cache hit") { figure(state.cacheHit) }
      }
      if !state.costs.isEmpty {
        InspectorSection("Cost by turn") {
          KeyValueGrid(columns: state.costColumns, rows: state.costs)
        }
      }
      if !state.budget.isEmpty {
        InspectorSection("Budget") {
          VStack(alignment: .leading, spacing: Space.m) {
            ForEach(state.budget, id: \.label) { budget in
              VStack(alignment: .leading, spacing: Space.xs) {
                HStack {
                  Text(budget.label).foregroundStyle(Color(.textPrimary))
                  Spacer(minLength: Space.s)
                  Text(budget.text).foregroundStyle(Color(.textSecondary))
                }
                .textStyle(.caption, tabularDigits: true)
                if let fraction = budget.fraction { gauge(fraction) }
              }
              .accessibilityElement(children: .combine)
            }
          }
        }
      }
      if !state.footnote.isEmpty {
        Text(state.footnote)
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .fixedSize(horizontal: false, vertical: true)
      }
      if state.parts.isEmpty && state.cacheHit.isEmpty && state.costs.isEmpty
        && state.budget.isEmpty
      {
        Text("No context yet")
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
      }
    }
  }

  /// A swatch, the part and its tokens per row; the free share last and quieter.
  private var legend: some View {
    Grid(alignment: .leading, horizontalSpacing: Space.s, verticalSpacing: Space.xs) {
      ForEach(state.parts, id: \.kind) { part in
        row(part.kind.colour, part.label, part.tokens, text: Color(.textPrimary))
      }
      if !state.free.isEmpty {
        row(Color(.fillSecondary), "Free", state.free, text: Color(.textSecondary))
      }
    }
    // The mockup's `.legend` is 12 pt, a size up from a footnote.
    .textStyle(.caption, tabularDigits: true)
  }

  private func row(_ swatch: Color, _ label: String, _ tokens: String, text: Color) -> some View {
    GridRow {
      RoundedRectangle(cornerRadius: Radius.swatch).fill(swatch)
        .frame(width: Self.swatch, height: Self.swatch)
      Text(label).foregroundStyle(text).frame(maxWidth: .infinity, alignment: .leading)
      Text(tokens).foregroundStyle(Color(.textSecondary)).gridColumnAlignment(.trailing)
    }
  }

  /// The share of a cap spent: an `accent` capsule over `fill.secondary`, as `ProgressRing` draws
  /// a share, never past its end.
  private func gauge(_ fraction: Double) -> some View {
    let share = fraction.isNaN ? 0 : min(max(fraction, 0), 1)
    return GeometryReader { proxy in
      Rectangle().fill(Color(.accent)).frame(width: proxy.size.width * share)
    }
    .frame(height: Self.gaugeHeight)
    .background(Color(.fillSecondary))
    .clipShape(Capsule())
    .accessibilityValue(Text(share, format: .percent.precision(.fractionLength(0))))
  }

  /// A header's trailing figure.
  private func figure(_ text: String) -> some View {
    Text(text)
      .textStyle(.label, tabularDigits: true)
      .foregroundStyle(Color(.textSecondary))
  }
}

#Preview("context") { PreviewMatrix { ContextInspectorSample(state: PreviewState.contextTab) } }
#Preview("costs") { PreviewMatrix { ContextInspectorSample(state: PreviewState.contextCosts) } }
#Preview("budget") { PreviewMatrix { ContextInspectorSample(state: PreviewState.contextBudget) } }
#Preview("turn running") {
  PreviewMatrix { ContextInspectorSample(state: PreviewState.contextRunning) }
}
#Preview("no window") {
  PreviewMatrix { ContextInspectorSample(state: PreviewState.contextNoWindow) }
}
#Preview("empty") { PreviewMatrix { ContextInspectorSample(state: .init()) } }
