// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TerminalTail` (DS§6.3 row `TerminalTail`, the mockup's `.tail`): the last lines a command
// printed, in a dark sunken well under its tool row, and how it exited. Separate so a running
// shell call and a finished one show their output the same way; the core picks and sanitizes
// the lines, the view only draws them.

import SwiftUI

/// The lines in `font.mono.terminal` and `text.terminal`, each cut with an ellipsis, in an
/// `insetWell`; after exit a check and the status in `text.terminalOk`, or a cross in
/// `status.danger` and the status in `text.terminal`.
public struct TerminalTail: View {
  public enum Exit: Equatable, Sendable {
    /// Still running: no exit line yet.
    case running
    /// Exited cleanly, with the core's status, `exit 0 · 9.8 s`.
    case succeeded(String)
    case failed(String)
  }

  let lines: [String]
  let exit: Exit

  init(_ lines: [String], exit: Exit) {
    self.lines = lines
    self.exit = exit
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.xxs) {
      ForEach(lines.indices, id: \.self) { index in
        Text(lines[index]).truncationMode(.tail)
      }
      TerminalExit(exit: exit)
    }
    .textStyle(.monoTerminal)
    .foregroundStyle(Color(.textTerminal))
    .lineLimit(1)
    .padding(.horizontal, Space.ml)
    .padding(.vertical, Space.m)
    .frame(maxWidth: .infinity, alignment: .leading)
    .insetWell(Color(.surfaceTerminal), cornerRadius: Radius.m)
  }
}

/// The exit line, drawn only once the command has exited.
private struct TerminalExit: View {
  let exit: TerminalTail.Exit

  var body: some View {
    switch exit {
    case .running: EmptyView()
    case .succeeded(let status):
      TerminalExitLine(
        symbol: "checkmark", colour: Color(.textTerminalOk), status: status,
        text: Color(.textTerminalOk))
    case .failed(let status):
      TerminalExitLine(
        symbol: "xmark", colour: Color(.statusDanger), status: status, text: Color(.textTerminal))
    }
  }
}

/// The glyph in its colour, then the status; `status.danger` stays on the glyph, where 3:1 is
/// enough, and off the text, which it would leave below 4.5:1 on the terminal well (DS§8).
private struct TerminalExitLine: View {
  let symbol: String
  let colour: Color
  let status: String
  let text: Color

  var body: some View {
    HStack(spacing: Space.s) {
      Image(systemName: symbol).symbolStyle(.monoTerminal).foregroundStyle(colour)
      Text(status).foregroundStyle(text)
    }
    .padding(.top, Space.xs)
  }
}

#Preview("running") { PreviewMatrix { TerminalTailSample(exit: .running) } }
#Preview("succeeded") { PreviewMatrix { TerminalTailSample(exit: PreviewState.tailSucceeded) } }
#Preview("failed") { PreviewMatrix { TerminalTailSample(exit: PreviewState.tailFailed) } }
