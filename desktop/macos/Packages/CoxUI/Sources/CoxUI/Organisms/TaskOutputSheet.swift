// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TaskOutputSheet` (DS§6.4 row `TasksTab`, its shell row's detail; DT§5.1; T37.22.6): the whole
// output a background shell left in the archive, the lossless copy `cox expand <id>` prints, in the
// same dark well `TerminalTail` draws its last lines in. Separate so the Tasks tab only opens it;
// the core reads and sanitizes the text, the sheet only draws and scrolls it.

import SwiftUI

/// The task's label over its output in `font.mono.terminal` and `text.terminal`, selectable and
/// scrolling in an `insetWell` of `surface.terminal`; Done or Esc closes it.
public struct TaskOutputSheet: View {
  let title: String
  let output: String
  let close: () -> Void

  public init(title: String, output: String, close: @escaping () -> Void) {
    (self.title, self.output, self.close) = (title, output, close)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.l) {
      HStack {
        Text(title).textStyle(.body).fontWeight(.semibold)
          .foregroundStyle(Color(.textPrimary))
          .lineLimit(1)
          .truncationMode(.middle)
          .accessibilityAddTraits(.isHeader)
        Spacer(minLength: Space.m)
        Button("Done", action: close)
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          .keyboardShortcut(.cancelAction)
      }
      ScrollView {
        Text(output.isEmpty ? "The task printed nothing." : output)
          .textStyle(.monoTerminal)
          .foregroundStyle(Color(.textTerminal))
          .textSelection(.enabled)
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(.horizontal, Space.ml)
          .padding(.vertical, Space.m)
      }
      .insetWell(Color(.surfaceTerminal), cornerRadius: Radius.m)
    }
    .padding(Space.xl)
    .frame(width: Size.readingWidth, height: Size.windowMinHeight - Size.toolbarHeight)
  }
}

#Preview("output") {
  PreviewMatrix {
    TaskOutputSheet(title: "cargo build", output: PreviewState.taskOutput) {}
  }
}
#Preview("empty") { PreviewMatrix { TaskOutputSheet(title: "true", output: "") {} } }
