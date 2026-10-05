// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ToolHeader` (DS§6.3 row `ToolHeader`, the mockup's `.tool .h`): one tool call at a glance —
// what kind of work, what it did to what, how risky, whether it is still running and how long
// it took, and whether its body is open. Separate so a collapsed tool row and an expanded
// tool card share one header, and the card's body (diff, terminal, code) stays its own part.

import SwiftUI

/// An `IconTile`, the summary with its subject in bold, a `DiffStat`, a `RiskChip`, the state
/// and duration in `text.secondary` and a disclosure chevron. The expanded header sits on
/// `fill.primary` over a hairline, the mockup's `.tool.exp .h`; a collapsed one is flat (e0).
public struct ToolHeader: View {
  /// What the header shows; every string comes formatted from the core.
  public struct Item: Equatable, Sendable {
    public var tile: IconTile.Kind
    /// An SF Symbol from the DS§3.7 map.
    public var symbol: String
    /// What the tool did, `Edited`.
    public var verb: String
    /// What it did it to, `crates/cox-provider-http/src/retry.rs`, drawn in bold.
    public var subject: String
    /// A command rather than a name: drawn monospaced, the mockup's `b.mono`.
    public var subjectIsCode: Bool
    /// More in `text.secondary`, `· retry.rs, http.rs +2`.
    public var detail: String?
    public var change: Change?
    public var risk: Risk?
    public var state: State
    /// How long it ran, `0.1 s`.
    public var duration: String?

    public init(
      tile: IconTile.Kind, symbol: String, verb: String, subject: String,
      subjectIsCode: Bool = false, detail: String? = nil, change: Change? = nil, risk: Risk? = nil,
      state: State, duration: String? = nil
    ) {
      (self.tile, self.symbol, self.verb, self.subject) = (tile, symbol, verb, subject)
      (self.subjectIsCode, self.detail, self.change, self.risk) = (
        subjectIsCode, detail, change, risk
      )
      (self.state, self.duration) = (state, duration)
    }
  }

  /// The lines a file change adds and removes.
  public struct Change: Equatable, Sendable {
    public var added: Int
    public var removed: Int

    public init(added: Int, removed: Int) {
      self.added = added
      self.removed = removed
    }
  }

  public struct Risk: Equatable, Sendable {
    public var text: String
    public var level: RiskChip.Level

    public init(text: String, level: RiskChip.Level) {
      self.text = text
      self.level = level
    }
  }

  public enum State: CaseIterable, Sendable {
    case running, succeeded, failed
  }

  let item: Item
  /// `nil` for a call with no body to open, which draws no chevron and is not a button.
  let isExpanded: Bool?
  let toggle: () -> Void

  init(_ item: Item, isExpanded: Bool? = nil, toggle: @escaping () -> Void = {}) {
    self.item = item
    self.isExpanded = isExpanded
    self.toggle = toggle
  }

  public var body: some View {
    if let isExpanded {
      Button(action: toggle) { ToolHeaderRow(item: item, isExpanded: isExpanded) }
        .buttonStyle(ToolHeaderStyle(isExpanded: isExpanded))
        .accessibilityValue(isExpanded ? "Expanded" : "Collapsed")
    } else {
      ToolHeaderRow(item: item, isExpanded: nil).modifier(ToolHeaderFace(isExpanded: false))
    }
  }
}

/// The header's content, laid out as the mockup's `.tool .h` flex row.
private struct ToolHeaderRow: View {
  let item: ToolHeader.Item
  let isExpanded: Bool?

  var body: some View {
    HStack(spacing: Space.m) {
      IconTile(item.tile, symbol: item.symbol)
      summary
        .lineLimit(1)
        .truncationMode(.middle)
        .foregroundStyle(Color(.textPrimary))
      if let change = item.change { DiffStat(added: change.added, removed: change.removed) }
      Spacer(minLength: 0)
      if let risk = item.risk { RiskChip(risk.text, level: risk.level) }
      ToolHeaderStatus(state: item.state, duration: item.duration)
      if let isExpanded {
        Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
          .symbolStyle(.footnote)
          .foregroundStyle(Color(.textTertiary))
          .accessibilityHidden(true)
      }
    }
    .textStyle(.body)
    .accessibilityElement(children: .combine)
  }

  private var summary: Text {
    let subject = Text(item.subject).fontWeight(item.subjectIsCode ? .medium : .semibold)
    let styled = item.subjectIsCode ? subject.monospaced() : subject
    guard let detail = item.detail else { return Text("\(item.verb) \(styled)") }
    let rest = Text(detail).foregroundStyle(Color(.textSecondary))
    return Text("\(item.verb) \(styled) \(rest)")
  }
}

/// A spinner, check or cross, then the duration, the mockup's `.st`; a task row shows it too.
struct ToolHeaderStatus: View {
  let state: ToolHeader.State
  let duration: String?

  var body: some View {
    HStack(spacing: Space.s) {
      switch state {
      case .running: Spinner()
      case .succeeded:
        ToolHeaderMark(symbol: "checkmark", colour: Color(.statusSuccess), label: "Done")
      case .failed: ToolHeaderMark(symbol: "xmark", colour: Color(.statusDanger), label: "Failed")
      }
      if let duration {
        Text(duration).textStyle(.footnote, tabularDigits: true)
      }
    }
    .foregroundStyle(Color(.textSecondary))
  }
}

/// A finished call's glyph, named for VoiceOver.
private struct ToolHeaderMark: View {
  let symbol: String
  let colour: Color
  let label: String

  var body: some View {
    Image(systemName: symbol).symbolStyle(.caption).foregroundStyle(colour).accessibilityLabel(
      label)
  }
}

/// Hover and press lay the usual quiet fills over the header (`ControlState`).
private struct ToolHeaderStyle: ButtonStyle {
  let isExpanded: Bool

  func makeBody(configuration: Configuration) -> some View {
    ControlStateReader(isPressed: configuration.isPressed) { state in
      configuration.label
        .modifier(ToolHeaderFace(isExpanded: isExpanded, tint: state.tint))
    }
  }
}

/// The mockup's padding and radius; an expanded header takes `fill.primary`, rounds only its
/// top corners to meet the card and ends on a hairline.
private struct ToolHeaderFace: ViewModifier {
  let isExpanded: Bool
  var tint: Color = .clear

  func body(content: Content) -> some View {
    let shape = UnevenRoundedRectangle(
      topLeadingRadius: Radius.l, bottomLeadingRadius: isExpanded ? 0 : Radius.l,
      bottomTrailingRadius: isExpanded ? 0 : Radius.l, topTrailingRadius: Radius.l,
      style: .continuous)
    content
      .padding(.horizontal, Space.ml)
      .padding(.vertical, Space.s)
      .background { shape.fill(tint) }
      .background { if isExpanded { shape.fill(Color(.fillPrimary)) } }
      .hairline(isExpanded ? .bottom : [])
      .contentShape(shape)
  }
}

#Preview("edited") {
  PreviewMatrix { ToolHeaderSample(PreviewState.toolEdited, isExpanded: false) }
}
#Preview("expanded") {
  PreviewMatrix { ToolHeaderSample(PreviewState.toolEdited, isExpanded: true) }
}
#Preview("running") { PreviewMatrix { ToolHeaderSample(PreviewState.toolRunning) } }
#Preview("explored") {
  PreviewMatrix { ToolHeaderSample(PreviewState.toolExplored, isExpanded: false) }
}
#Preview("failed") { PreviewMatrix { ToolHeaderSample(PreviewState.toolFailed) } }
