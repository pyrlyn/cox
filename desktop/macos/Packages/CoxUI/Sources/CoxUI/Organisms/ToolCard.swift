// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ToolCard` (DS§6.4 row `ToolCard`, the mockup's `.tool` and `.tool.exp`): one tool call in the
// transcript — its header and, while it runs or once opened, one detail under it: the diff it
// made or the last lines it printed. Separate so the transcript's card attachments (T37.23) and
// any later list of calls draw a call the same way from the DS§6.3 molecules.

import SwiftUI

/// A running call shows its detail under a flat header; a finished one folds the detail behind
/// the header's chevron. Opened, the card is a readable face at e2 with a hairline rim, the
/// mockup's `.tool.exp`; folded, it is the flat header alone. Open or folded is the card's own
/// state (DS§9), so it survives the transcript laying the card out again.
public struct ToolCard: View {
  /// What the card shows; every string comes formatted from the core.
  public struct Content: Equatable, Sendable {
    public var header: ToolHeader.Item
    public var detail: Detail?

    public init(header: ToolHeader.Item, detail: Detail? = nil) {
      self.header = header
      self.detail = detail
    }
  }

  /// The one body a call has.
  public enum Detail: Equatable, Sendable {
    /// The hunks an edit made.
    case diff([Hunk])
    /// The last lines a command printed and how it exited.
    case tail([String], exit: TerminalTail.Exit)
    /// A plugin's `tool:` tree; when set it is the body instead of the generic diff or tail.
    case plugin(PluginWidget)
  }

  /// One hunk: the core's `@@` header and its lines.
  public struct Hunk: Equatable, Sendable {
    public var header: String
    public var lines: [DiffLineView.Line]

    public init(header: String, lines: [DiffLineView.Line]) {
      self.header = header
      self.lines = lines
    }
  }

  let content: Content
  @State private var isExpanded: Bool

  public init(_ content: Content, isExpanded: Bool = false) {
    self.content = content
    self._isExpanded = State(initialValue: isExpanded)
  }

  public var body: some View {
    let running = content.header.state == .running
    let shown = content.detail.flatMap { running || isExpanded ? $0 : nil }
    VStack(alignment: .leading, spacing: running ? Space.xs : 0) {
      // A running call's detail is always shown, so only a finished one has a chevron.
      ToolHeader(content.header, isExpanded: content.detail == nil || running ? nil : isExpanded) {
        isExpanded.toggle()
      }
      if let shown { ToolCardDetail(detail: shown).coxTransition(.opacity) }
    }
    .modifier(ToolCardFace(isOpen: shown != nil && !running))
    .animation(.cox(Motion.durationBase), value: isExpanded)
  }
}

/// The diff's hunks, or the terminal tail.
private struct ToolCardDetail: View {
  let detail: ToolCard.Detail

  var body: some View {
    switch detail {
    case .diff(let hunks):
      VStack(alignment: .leading, spacing: 0) {
        ForEach(hunks.indices, id: \.self) {
          DiffHunkView(header: hunks[$0].header, lines: hunks[$0].lines)
        }
      }
    case .tail(let lines, let exit):
      TerminalTail(lines, exit: exit)
    case .plugin(let widget):
      PluginWidgetView(widget)
    }
  }
}

/// An opened card: `surface.window` over the readable floor, a hairline rim, `radius.l` and
/// e2 (the mockup's `.tool.exp`). The diff's code surface is clipped to the same corners.
private struct ToolCardFace: ViewModifier {
  let isOpen: Bool

  func body(content: Content) -> some View {
    if isOpen {
      let shape = RoundedRectangle(cornerRadius: Radius.l, style: .continuous)
      content
        .clipShape(shape)
        .background { Color.clear.glassPane(shape, role: .readable) }
        .hairline(in: shape)
        .elevation(.e2, cornerRadius: Radius.l)
    } else {
      content
    }
  }
}

#Preview("edited") { PreviewMatrix { ToolCardSample(PreviewState.cardEdited) } }
#Preview("edited, open") {
  PreviewMatrix { ToolCardSample(PreviewState.cardEdited, isExpanded: true) }
}
#Preview("running") { PreviewMatrix { ToolCardSample(PreviewState.cardRunning) } }
#Preview("failed, open") {
  PreviewMatrix { ToolCardSample(PreviewState.cardFailed, isExpanded: true) }
}
#Preview("explored") { PreviewMatrix { ToolCardSample(PreviewState.cardExplored) } }
#Preview("plugin, open") {
  PreviewMatrix { ToolCardSample(PreviewState.cardPlugin, isExpanded: true) }
}
