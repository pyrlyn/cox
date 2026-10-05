// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TokenPopover` (DS§6.4 row `TokenPopover`, the mockup's `.tokpop`; DS§7; mockup screen 30):
// where the meter's tokens went — the turn's rate with its sparkline, first-token time, sent and
// received for the turn and the session with their breakdowns, cost, and the context bar.
// Separate so the composer opens it from one value; every figure arrives formatted by `cox-app`.

import SwiftUI

/// A heading with the turn's phase, the big tok/s over its sparkline and a rate line, the
/// turn / session KeyValueGrid, the context heading over a StackedBar with its legend, and a
/// footnote, on readable popover glass at e4.
public struct TokenPopover: View {
  public struct State: Equatable, Sendable {
    /// `This turn · 4 requests` and `streaming`.
    public var heading = "", phase = ""
    /// `71`, `tok/s now`, `avg 64 tok/s · first token 800 ms · peak 77 tok/s`.
    public var rate = "", rateUnit = "", rateDetail = ""
    /// The phase in `accent` while a turn runs.
    public var isStreaming = false
    public var sparkline: [Double] = []
    public var rows: [Row] = []
    /// `Context · 76.4k` and its share of the window, `38%` (empty when unknown).
    public var context = "", contextShare = ""
    /// The context window's parts; none hides the bar.
    public var parts: [Part] = []
    public var footnote = ""

    public init() {}
  }

  /// A grid line: the label and its turn and session figures.
  public struct Row: Equatable, Sendable {
    public var label, turn, session: String
    /// A breakdown of the row above it.
    public var isDetail: Bool

    public init(label: String, turn: String, session: String, isDetail: Bool = false) {
      (self.label, self.turn, self.session, self.isDetail) = (label, turn, session, isDetail)
    }
  }

  /// A part of the context window: its share of the bar and its legend, `system 6.2k`.
  public struct Part: Equatable, Sendable {
    public var kind: StackedBar.Kind
    public var fraction: Double
    public var legend: String

    public init(kind: StackedBar.Kind, fraction: Double, legend: String) {
      (self.kind, self.fraction, self.legend) = (kind, fraction, legend)
    }
  }

  let state: State

  /// The mockup's popover sparkline, 150 × 34.
  private static let sparkWidth: CGFloat = 150
  private static let sparkHeight: CGFloat = 34
  /// The mockup's legend swatch, 8 pt.
  private static let swatch: CGFloat = 8

  public init(state: State) {
    self.state = state
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.popover, style: .continuous)
    VStack(alignment: .leading, spacing: Space.l) {
      if !state.heading.isEmpty {
        rate
      }
      KeyValueGrid(
        columns: ["Turn", "Session"],
        rows: state.rows.map {
          KeyValueGrid.Row(label: $0.label, values: [$0.turn, $0.session], isDetail: $0.isDetail)
        })
      context
      // The mockup's 11 pt regular note.
      Text(state.footnote)
        .textStyle(.detail)
        .foregroundStyle(Color(.textTertiary))
        .fixedSize(horizontal: false, vertical: true)
    }
    .padding(.horizontal, Space.xl)
    .padding(.vertical, Space.popover)
    .frame(width: Size.tokenPopoverWidth)
    .glassPane(shape, surface: Color(.surfacePopover), role: .readable)
    .hairline(in: shape)
    .elevation(.e4, cornerRadius: Radius.popover)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Tokens")
  }

  /// The heading and phase, the big rate beside its sparkline, and the rate line.
  private var rate: some View {
    VStack(alignment: .leading, spacing: Space.s) {
      // The mockup's `THIS TURN · 4 REQUESTS` over a lowercase `streaming`.
      header(state.heading, trailing: state.phase, accent: state.isStreaming, uppercased: true)
      HStack(alignment: .lastTextBaseline, spacing: Space.s) {
        Text(state.rate).textStyle(.metric).foregroundStyle(Color(.textPrimary))
        Text(state.rateUnit).textStyle(.caption).foregroundStyle(Color(.textSecondary))
        Spacer(minLength: Space.l)
        Sparkline(state.sparkline).frame(width: Self.sparkWidth, height: Self.sparkHeight)
      }
      Text(state.rateDetail).textStyle(.detail, tabularDigits: true)
        .foregroundStyle(Color(.textSecondary))
    }
  }

  /// The context heading and share, then the bar and its legend when the parts are known.
  private var context: some View {
    VStack(alignment: .leading, spacing: Space.s) {
      header(state.context, trailing: state.contextShare, accent: false)
      if !state.parts.isEmpty {
        StackedBar(state.parts.map { .init(kind: $0.kind, fraction: $0.fraction) })
        // The mockup's legend wraps: on one line when it fits, else in two.
        ViewThatFits(in: .horizontal) {
          legend(state.parts)
          VStack(alignment: .leading, spacing: Space.xs) {
            let half = (state.parts.count + 1) / 2
            legend(Array(state.parts.prefix(half)))
            legend(Array(state.parts.dropFirst(half)))
          }
        }
        .textStyle(.legend, tabularDigits: true)
        .foregroundStyle(Color(.textSecondary))
      }
    }
  }

  /// Each part's swatch and legend, in a row.
  private func legend(_ parts: [Part]) -> some View {
    HStack(spacing: Space.m) {
      ForEach(parts, id: \.legend) { part in
        HStack(spacing: Space.xs) {
          RoundedRectangle(cornerRadius: Radius.swatch).fill(part.kind.colour)
            .frame(width: Self.swatch, height: Self.swatch)
          Text(part.legend).fixedSize()
        }
      }
    }
  }

  /// The mockup's `h5`: a label with its value at the trailing edge.
  private func header(
    _ title: String, trailing: String, accent: Bool, uppercased: Bool = false
  ) -> some View {
    HStack {
      Text(title).textCase(uppercased ? .uppercase : nil).accessibilityAddTraits(.isHeader)
      Spacer(minLength: Space.m)
      Text(trailing).foregroundStyle(Color(accent ? .accent : .textSecondary))
    }
    .textStyle(.label, tabularDigits: true)
    .foregroundStyle(Color(.textSecondary))
  }
}

#Preview("streaming") { PreviewMatrix { TokenPopover(state: PreviewState.tokensStreaming) } }
#Preview("idle") { PreviewMatrix { TokenPopover(state: PreviewState.tokensIdle) } }
#Preview("live") { PreviewMatrix { TokenPopover(state: PreviewState.tokensLive) } }
