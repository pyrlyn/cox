// `TurnGutter` (Figma frames 01-main-session-streaming and 14-rewind-edit-resend, the mockup's
// `.gutter`): a prompt's turn number in the margin left of its bubble. A marked turn — the one a
// rewind would go back to before — shows the number in accent with the rewind glyph and opens
// `RewindMenu`. Separate so the transcript (SwiftUI samples and the AppKit text view alike)
// numbers turns one way.

import SwiftUI

/// `Size.turnGutter` wide, right-aligned, `Size.turnGutterOffset` from its leading edge to the
/// bubble; `font.detail` with tabular digits.
public struct TurnGutter: View {
  let turn: UInt32
  let isMarked: Bool
  let open: (() -> Void)?

  /// `open` makes the number a button that opens the turn's rewind menu; `nil` leaves a label.
  public init(turn: UInt32, isMarked: Bool = false, open: (() -> Void)? = nil) {
    (self.turn, self.isMarked, self.open) = (turn, isMarked, open)
  }

  public var body: some View {
    if let open {
      Button(action: open) { label }
        .buttonStyle(.plain)
        .help("Rewind to before turn \(turn)")
        .accessibilityLabel("Turn \(turn), rewind")
    } else {
      label.accessibilityLabel("Turn \(turn)")
    }
  }

  private var label: some View {
    HStack(spacing: Space.xxs) {
      Text(verbatim: "\(turn)")
      if isMarked { Image(systemName: "arrow.uturn.backward").symbolStyle(.detail) }
    }
    .textStyle(.detail, tabularDigits: true)
    .fontWeight(isMarked ? .semibold : .regular)
    // `text.secondary`, not the mockup's tertiary: the number must stay readable on frosted
    // glass (DS§8).
    .foregroundStyle(isMarked ? Color(.accent) : Color(.textSecondary))
    .frame(width: Size.turnGutter, alignment: .trailing)
    .contentShape(Rectangle())
  }
}

#Preview("turn") { PreviewMatrix { TurnGutter(turn: 1) } }
#Preview("marked") { PreviewMatrix { TurnGutter(turn: 2, isMarked: true) {} } }
