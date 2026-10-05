// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PluginWidgetView` (DS§6.3 row `PluginWidgetView`, PL§8, T52.16): a plugin's closed widget
// tree — text, list, table, key–value rows, gauge, stack, block — drawn natively. Separate so a
// panel, a status segment, an overlay and a tool card draw one tree the same way. A plugin names
// colour roles, never colours: each role maps to a DS§3.1 token here, so light, dark and Increase
// Contrast keep working for plugin output. The app hands over values cox-app already sanitized
// and bounded; CoxUI imports no cox package.

import SwiftUI

/// A run of plugin text in one colour role.
public struct PluginSpan: Equatable, Sendable {
  /// PL§8's `StyleToken`: the TUI theme's roles a plugin may name. The permission-mode tints are
  /// not among them, so a plugin cannot imitate the composer's trust signal.
  public enum Role: CaseIterable, Sendable {
    // The token's own name, as CoxClient's `StyleToken` spells it.
    // swiftlint:disable:next identifier_name
    case text, dim, accent, user, agent, tool, ok, warn, error
    case diffAdd, diffDel, diffHunk, border, selection
  }

  public var text: String
  public var role: Role
  public var isBold: Bool
  public var isItalic: Bool

  public init(_ text: String, _ role: Role = .text, isBold: Bool = false, isItalic: Bool = false) {
    (self.text, self.role, self.isBold, self.isItalic) = (text, role, isBold, isItalic)
  }

  /// A line of spans as one attributed string, so one `Text` draws it.
  static func attributed(_ spans: [PluginSpan]) -> AttributedString {
    var line = AttributedString()
    for span in spans { line.append(span.attributed) }
    return line
  }

  private var attributed: AttributedString {
    var run = AttributedString(text)
    run.foregroundColor = role.colour
    var intent: InlinePresentationIntent = []
    if isBold { intent.insert(.stronglyEmphasized) }
    if isItalic { intent.insert(.emphasized) }
    run.inlinePresentationIntent = intent
    return run
  }
}

extension PluginSpan.Role {
  /// The DS§3.1 token a role draws in. Plan mode's `status.plan` is left unused on purpose.
  var colour: Color {
    switch self {
    case .text, .agent: Color(.textPrimary)
    case .dim, .border: Color(.textSecondary)
    case .accent, .selection: Color(.accent)
    case .user: Color(.roleProject)
    case .tool: Color(.syntaxFunction)
    case .diffHunk: Color(.syntaxKeyword)
    case .ok, .diffAdd: Color(.statusSuccess)
    case .warn: Color(.statusWarning)
    case .error, .diffDel: Color(.statusDanger)
    }
  }
}

/// PL§8's closed tree as plain values. The cell widths and stack sizes a terminal lays out by
/// are left out: the app lays the tree out natively.
public indirect enum PluginWidget: Equatable, Sendable {
  case text([[PluginSpan]])
  case list(items: [[PluginSpan]], selected: Int?)
  case table(header: [PluginSpan], rows: [[PluginSpan]])
  case keyValue([KeyValue])
  /// `ratio` in `0...1`.
  case gauge(ratio: Double, label: PluginSpan)
  case stack(vertical: Bool, children: [PluginWidget])
  case block(title: PluginSpan?, child: PluginWidget)

  /// One `key: value` row.
  public struct KeyValue: Equatable, Sendable {
    public var key: PluginSpan
    public var value: [PluginSpan]

    public init(key: PluginSpan, value: [PluginSpan]) {
      (self.key, self.value) = (key, value)
    }
  }
}

/// One widget tree, one view per variant, in the monospaced code face a plugin lays out for.
public struct PluginWidgetView: View {
  let widget: PluginWidget

  public init(_ widget: PluginWidget) {
    self.widget = widget
  }

  public var body: some View {
    Group {
      switch widget {
      case .text(let lines): PluginLines(lines: lines)
      case .list(let items, let selected): PluginList(items: items, selected: selected)
      case .table(let header, let rows): PluginTable(header: header, rows: rows)
      case .keyValue(let rows): PluginKeyValues(rows: rows)
      case .gauge(let ratio, let label): PluginGauge(ratio: ratio, label: label)
      case .stack(let vertical, let children): PluginStack(vertical: vertical, children: children)
      case .block(let title, let child): PluginBlock(title: title, child: child)
      }
    }
    .textStyle(.monoCode)
    .foregroundStyle(Color(.textPrimary))
  }
}

private struct PluginLines: View {
  let lines: [[PluginSpan]]

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
        Text(PluginSpan.attributed(line))
      }
    }
  }
}

/// The selected row sits on `accent.soft`, as a picker's does.
private struct PluginList: View {
  let items: [[PluginSpan]]
  let selected: Int?

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ForEach(Array(items.enumerated()), id: \.offset) { index, item in
        let isSelected = index == selected
        Text(PluginSpan.attributed(item))
          .padding(.horizontal, Space.xs)
          .frame(maxWidth: .infinity, alignment: .leading)
          .background(
            isSelected ? Color(.accentSoft) : Color.clear,
            in: RoundedRectangle(cornerRadius: Radius.xs, style: .continuous)
          )
          .accessibilityAddTraits(isSelected ? .isSelected : [])
      }
    }
  }
}

/// Styles go on the cells: a modifier on a `GridRow` would turn it into one spanning view.
private struct PluginTable: View {
  let header: [PluginSpan]
  let rows: [[PluginSpan]]

  var body: some View {
    Grid(alignment: .leading, horizontalSpacing: Space.l, verticalSpacing: Space.xxs) {
      if !header.isEmpty {
        GridRow {
          ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
            Text(PluginSpan.attributed([cell])).fontWeight(.semibold)
          }
        }
      }
      ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
        GridRow {
          ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
            Text(PluginSpan.attributed([cell]))
          }
        }
      }
    }
  }
}

private struct PluginKeyValues: View {
  let rows: [PluginWidget.KeyValue]

  var body: some View {
    Grid(
      alignment: .leadingFirstTextBaseline, horizontalSpacing: Space.l, verticalSpacing: Space.xxs
    ) {
      ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
        GridRow {
          Text(PluginSpan.attributed([row.key]))
          Text(PluginSpan.attributed(row.value))
            .frame(maxWidth: .infinity, alignment: .leading)
        }
      }
    }
  }
}

private struct PluginGauge: View {
  let ratio: Double
  let label: PluginSpan

  var body: some View {
    HStack(spacing: Space.m) {
      if !label.text.isEmpty { Text(PluginSpan.attributed([label])) }
      ProgressView(value: ratio.isNaN ? 0 : min(max(ratio, 0), 1))
        .progressViewStyle(.linear)
        .tint(Color(.accent))
    }
  }
}

private struct PluginStack: View {
  let vertical: Bool
  let children: [PluginWidget]

  var body: some View {
    if vertical {
      VStack(alignment: .leading, spacing: Space.xs) { items }
    } else {
      HStack(alignment: .top, spacing: Space.l) { items }
    }
  }

  private var items: some View {
    ForEach(Array(children.enumerated()), id: \.offset) { _, child in PluginWidgetView(child) }
  }
}

/// PL§8's bordered box: a hairline-weight stroke in `separator`, the title above the content.
private struct PluginBlock: View {
  let title: PluginSpan?
  let child: PluginWidget

  var body: some View {
    VStack(alignment: .leading, spacing: Space.xs) {
      if let title { Text(PluginSpan.attributed([title])).fontWeight(.semibold) }
      PluginWidgetView(child)
    }
    .padding(Space.m)
    .frame(maxWidth: .infinity, alignment: .leading)
    .overlay(
      RoundedRectangle(cornerRadius: Radius.s, style: .continuous)
        .strokeBorder(Color(.separator))
    )
  }
}

#Preview("variants") {
  PreviewMatrix {
    VStack(alignment: .leading, spacing: Space.l) {
      ForEach(PreviewState.pluginWidgets, id: \.name) { sample in PluginWidgetView(sample.widget) }
    }
    .frame(width: Size.popoverWidth)
  }
}
