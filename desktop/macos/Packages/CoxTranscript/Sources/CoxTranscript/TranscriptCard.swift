// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The card a card block shows (T37.23, DT§5.2): a tool call, a tool group or a subagent task
// as a CoxUI `ToolCard`, an approval or a question in the caller's slot (the `ApprovalCard`,
// T37.27). Separate so the one place that reads a block's fields to fill a card is its own
// file; every value it shows was decided in Rust — this maps enums to looks and lays out.

import CoxClient
import CoxUI
import SwiftUI

/// One card block's view, hosted in the transcript text as an attachment (T37.41).
struct TranscriptCard<Approval: View>: View {
  let block: Block
  let approval: @MainActor (Block) -> Approval
  @Environment(\.locale) private var locale

  var body: some View {
    if let content = ToolCard.Content(block, locale: locale) {
      ToolCard(content)
    } else {
      approval(block)
    }
  }
}

extension ToolCard.Content {
  /// The card of a tool call, tool group or task, its duration written as `locale` writes
  /// it; `nil` for any other block.
  init?(_ block: Block, locale: Locale) {
    switch block.kind {
    case .tool(
      _, let summary, let icon, let risk, let state, let tail, _, let diff, let durationMs,
      let pluginView):
      let duration = state == .running ? nil : Self.seconds(durationMs, locale)
      let header = ToolHeader.Item(
        tile: icon.tile, symbol: icon.symbol, verb: summary, subject: "", risk: risk.chip,
        state: state.header, duration: duration)
      // A plugin's own tree replaces the generic tail or diff; the fallback is unchanged.
      let detail =
        pluginView.map { ToolCard.Detail.plugin(PluginWidget($0)) } ?? diff.flatMap(Self.hunks)
        ?? Self.tail(tail, state: state, status: duration)
      self.init(header: header, detail: detail)
    case .toolGroup(let summary, _, let state):
      let header = ToolHeader.Item(
        tile: Icon.search.tile, symbol: Icon.search.symbol, verb: summary, subject: "",
        state: state.header)
      self.init(header: header)
    case .task(_, let label, let tier, _, _, _, let task, _):
      let state: ToolState =
        switch task {
        case .running: .running
        case .succeeded: .done
        case .failed: .failed
        }
      let header = ToolHeader.Item(
        tile: Icon.agent.tile, symbol: Icon.agent.symbol, verb: label, subject: "",
        detail: tier.rawValue, state: state.header)
      self.init(header: header)
    default:
      return nil
    }
  }

  /// An edit's hunks as the core split, numbered and highlighted them (T37.23.5); none when
  /// it changed nothing, so the card falls back to the tail.
  private static func hunks(_ diff: DiffModel) -> ToolCard.Detail? {
    let hunks = diff.hunks.map(ToolCard.Hunk.init)
    return hunks.isEmpty ? nil : .diff(hunks)
  }

  /// The core's five-line tail, one string per line; none when the call printed nothing.
  private static func tail(_ tail: String, state: ToolState, status: String?) -> ToolCard.Detail? {
    let lines = tail.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    guard lines.contains(where: { !$0.isEmpty }) else { return nil }
    let shown = lines.last == "" ? Array(lines.dropLast()) : lines
    let exit: TerminalTail.Exit =
      switch state {
      case .running: .running
      case .done: .succeeded(status ?? "")
      case .failed: .failed(status ?? "")
      }
    return .tail(shown, exit: exit)
  }

  /// A duration as `locale` writes it: under a second, whole milliseconds (`40ms`); at or
  /// above a second, at most one decimal, dropped when the rounded value is a whole second
  /// (`4s`, not `4.0s` or, in a comma-decimal locale, `4,0s` — T37.44.17).
  private static func seconds(_ milliseconds: UInt64, _ locale: Locale) -> String {
    guard milliseconds >= 1000 else {
      return Duration.milliseconds(milliseconds).formatted(
        .units(allowed: [.milliseconds], width: .narrow).locale(locale))
    }
    // Whether the value rounds to a whole second at one decimal's precision — checked on
    // the rounded tenths, not `milliseconds % 1000`, so 2949ms (-> 2.9s) and 2999ms (-> 3s,
    // no decimal) both land on the display the formatted seconds value would actually show.
    let isWholeSecond =
      (Double(milliseconds) / 100).rounded().truncatingRemainder(dividingBy: 10) == 0
    let fractionalPart: Duration.UnitsFormatStyle.FractionalPartDisplayStrategy =
      isWholeSecond ? .hide(rounded: .toNearestOrAwayFromZero) : .show(length: 1)
    return Duration.milliseconds(milliseconds).formatted(
      .units(allowed: [.seconds], width: .narrow, fractionalPart: fractionalPart)
        .locale(locale))
  }
}

extension ToolCard.Hunk {
  /// A hunk as the core split, numbered and highlighted it; the transcript's edit cards and
  /// Review draw the same lines.
  init(_ hunk: DiffHunk) {
    self.init(header: hunk.header, lines: hunk.lines.map(DiffLineView.Line.init))
  }
}

extension DiffLineView.Line {
  /// The gutter shows the new number, or the old one for a removed line. A run takes the
  /// session theme's colours (A95): the core highlighted it with the theme's dark (`rgb`) and
  /// light variant, and `CodeRun` draws the one the view's appearance asks for; a run with no
  /// colour stays plain. The core cut the spans at every changed word's edge (T37.23.11), so a
  /// span is marked whole by where it starts.
  init(_ line: DiffLine) {
    let kind: DiffLineView.Kind =
      switch line.kind {
      case .context: .context
      case .add: .added
      case .del: .removed
      }
    let number = (line.kind == .del ? line.old : line.new).map(String.init) ?? ""
    var offset: UInt32 = 0
    let runs = line.spans.map { span in
      defer { offset += UInt32(span.text.utf8.count) }
      let changed = line.words.contains { $0.start <= offset && offset < $0.end }
      let theme = span.rgb.map { CodeRun.Theme(light: span.light ?? $0, dark: $0) }
      return CodeRun(span.text, isChanged: changed, theme: theme)
    }
    self.init(kind: kind, number: number, runs: runs)
  }
}

extension Icon {
  /// The tile colour and DS§3.7 symbol per icon, as DT§5.2 lists them.
  var tile: IconTile.Kind {
    switch self {
    case .read: .write
    case .edit: .edit
    case .shell: .shell
    case .search: .search
    case .web, .todo, .ask, .agent, .mcp, .tool: .neutral
    }
  }

  var symbol: String {
    switch self {
    case .read: "doc.text"
    case .edit: "pencil"
    case .shell: "terminal"
    case .search: "magnifyingglass"
    case .web: "globe"
    case .todo: "checklist"
    case .ask: "text.bubble"
    case .agent: "person.2"
    case .mcp: "bolt"
    case .tool: "sparkle"
    }
  }
}

extension Risk {
  /// Only a destructive call carries a chip (DT§5.2: high-risk calls carry a risk badge).
  var chip: ToolHeader.Risk? {
    self == .destructive ? ToolHeader.Risk(text: rawValue, level: .high) : nil
  }
}

extension ToolState {
  var header: ToolHeader.State {
    switch self {
    case .running: .running
    case .done: .succeeded
    case .failed: .failed
    }
  }
}

extension PluginWidget {
  public init(_ view: PluginView) {
    let line = { (runs: [PluginRun]) in runs.map(PluginSpan.init) }
    switch view {
    case .text(let lines): self = .text(lines.map(line))
    case .list(let items, let selected):
      self = .list(items: items.map(line), selected: selected.map(Int.init))
    case .table(let header, let rows): self = .table(header: line(header), rows: rows.map(line))
    case .keyValue(let rows):
      self = .keyValue(rows.map { .init(key: PluginSpan($0.key), value: line($0.value)) })
    case .gauge(let ratio, let label): self = .gauge(ratio: ratio, label: PluginSpan(label))
    case .stack(let vertical, let children):
      self = .stack(vertical: vertical, children: children.map(PluginWidget.init))
    case .block(let title, let child):
      // The FFI carries the one child as a list; an empty block draws its title over nothing.
      self = .block(
        title: title.map(PluginSpan.init),
        child: child.first.map(PluginWidget.init) ?? .text([]))
    }
  }
}

extension PluginSpan {
  public init(_ run: PluginRun) {
    self.init(run.text, Role(run.token), isBold: run.bold, isItalic: run.italic)
  }
}

extension PluginSpan.Role {
  // One case per token: a mapping, not branching logic.
  // swiftlint:disable:next cyclomatic_complexity
  public init(_ token: StyleToken) {
    switch token {
    case .text: self = .text
    case .dim: self = .dim
    case .accent: self = .accent
    case .user: self = .user
    case .agent: self = .agent
    case .tool: self = .tool
    case .ok: self = .ok
    case .warn: self = .warn
    case .error: self = .error
    case .diffAdd: self = .diffAdd
    case .diffDel: self = .diffDel
    case .diffHunk: self = .diffHunk
    case .border: self = .border
    case .selection: self = .selection
    }
  }
}
