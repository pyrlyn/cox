// `RewindMenu` (Figma frame 14-rewind-edit-resend, the mockup's `.popover` under a marked
// `.gutter`): a prompt's rewind scopes — code and conversation, code only, conversation only —
// each with what it keeps or restores, then Fork a new session here. Separate from
// `RewindTimeline`, which lists every checkpoint in Review; this menu is one turn's, opened from
// its `TurnGutter`. The core does the rewind and the fork (`Intent.rewind`, `Intent.fork`).

import SwiftUI

/// A header over three scope rows and, past a separator, the fork row, on readable popover
/// glass at e4, `Size.rewindMenuWidth` wide. The highlighted row is on `accent`.
public struct RewindMenu: View {
  /// What a rewind can restore (DT§3).
  public enum Scope: CaseIterable, Equatable, Sendable {
    case codeAndConversation, code, conversation

    var title: String {
      switch self {
      case .codeAndConversation: "Code and conversation"
      case .code: "Code only"
      case .conversation: "Conversation only"
      }
    }

    /// DS§3.7's rewind, doc and comment, as `RewindTimeline`'s scopes.
    var symbol: String {
      switch self {
      case .codeAndConversation: "arrow.uturn.backward"
      case .code: "doc.text"
      case .conversation: "text.bubble"
      }
    }

    var restores: (code: Bool, conversation: Bool) {
      switch self {
      case .codeAndConversation: (true, true)
      case .code: (true, false)
      case .conversation: (false, true)
      }
    }
  }

  public struct State: Equatable, Sendable {
    /// The prompt's turn; the rewind goes back to before it.
    public var turn: UInt32
    /// How many files a code rewind restores; `nil` while unknown hides the count.
    public var restoredFiles: Int?
    /// The row under the pointer or the keyboard.
    public var highlight: Scope?

    public init(turn: UInt32, restoredFiles: Int? = nil, highlight: Scope? = .codeAndConversation) {
      (self.turn, self.restoredFiles, self.highlight) = (turn, restoredFiles, highlight)
    }
  }

  public enum Intent: Equatable, Sendable {
    case rewind(turn: UInt32, code: Bool, conversation: Bool)
    /// A new session that starts from the conversation before this turn.
    case fork(turn: UInt32)
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    self.state = state
    self.send = send
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xl, style: .continuous)
    VStack(alignment: .leading, spacing: 0) {
      SectionHeader("Rewind to before turn \(state.turn)")
        .padding(.horizontal, Space.l)
        .padding(.top, Space.s)
        .padding(.bottom, Space.xs)
      ForEach(Scope.allCases, id: \.self) { scope in
        RewindMenuRow(
          title: scope.title, symbol: scope.symbol, hint: hint(scope),
          isHighlighted: scope == state.highlight
        ) { send(Self.intent(scope, turn: state.turn)) }
      }
      Color(.separator)
        .frame(height: Size.hairline)
        .padding(.horizontal, Space.m)
        .padding(.vertical, Space.xs)
      RewindMenuRow(
        title: "Fork a new session here", symbol: "arrow.triangle.branch", hint: nil,
        isHighlighted: false
      ) { send(.fork(turn: state.turn)) }
    }
    .padding(Space.xs)
    .frame(width: Size.rewindMenuWidth, alignment: .leading)
    .glassPane(shape, surface: Color(.surfacePopover), role: .readable)
    .hairline(in: shape)
    .elevation(.e4, cornerRadius: Radius.xl)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Rewind to before turn \(state.turn)")
  }

  /// The rewind a scope's row reports.
  static func intent(_ scope: Scope, turn: UInt32) -> Intent {
    let restores = scope.restores
    return .rewind(turn: turn, code: restores.code, conversation: restores.conversation)
  }

  /// What a scope keeps or restores, as the frame words it.
  func hint(_ scope: Scope) -> String? {
    switch scope {
    case .codeAndConversation:
      state.restoredFiles.map { $0 == 1 ? "1 file restored" : "\($0) files restored" }
    case .code: "keep the chat"
    case .conversation: "keep the files"
    }
  }
}

/// The mockup's `.popover .it`: symbol, title, the hint pushed trailing.
struct RewindMenuRow: View {
  let title: String
  let symbol: String
  let hint: String?
  let isHighlighted: Bool
  let action: () -> Void

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.m, style: .continuous)
    Button(action: action) {
      HStack(spacing: Space.ml) {
        // One column for every symbol, so the titles line up whatever the glyph's width.
        Image(systemName: symbol).symbolStyle(.body).frame(width: Size.iconTile)
        Text(title).textStyle(.body).lineLimit(1)
        Spacer(minLength: Space.xl)
        if let hint {
          Text(hint)
            .textStyle(.footnote)
            .foregroundStyle(isHighlighted ? Color(.textOnAccent) : Color(.textSecondary))
            .lineLimit(1)
        }
      }
      .foregroundStyle(isHighlighted ? Color(.textOnAccent) : Color(.textPrimary))
      .padding(.horizontal, Space.l)
      .padding(.vertical, Space.s)
      .background { if isHighlighted { shape.fill(Color(.accent)) } }
      .contentShape(shape)
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(isHighlighted ? .isSelected : [])
  }
}

#Preview("menu") { PreviewMatrix { RewindMenu(state: PreviewState.rewindMenu) { _ in } } }
#Preview("count unknown") {
  PreviewMatrix { RewindMenu(state: .init(turn: 2, highlight: nil)) { _ in } }
}
