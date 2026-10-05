// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CommandPalette` (DS§6.4 row `CommandPalette`, mockup 12; DT§5.5, T37.44.13): ⌘K's one list
// over the window's actions, the sessions, slash commands and the project's files, under a query
// field. Separate so the window only places it over its scrim: which rows show, their order and
// the characters the query matched all come from `cox-app`'s palette ranking.

import SwiftUI

/// The query in `font.title.palette` over a `Hairline`, then a `SectionHeader` per section over
/// its rows on readable popover glass at e4. A row leads with its symbol in a `fill.secondary`
/// well, draws the matched characters of its title bold and its detail or keys in
/// `text.secondary` at the trailing edge; the selected row is the mockup's `.it.on`,
/// `text.onAccent` on an `accent` face.
public struct CommandPalette: View {
  public struct State: Equatable, Sendable {
    public var query: String
    public var sections: [Section]
    /// The row ⏎ runs.
    public var selection: Row.ID?

    public init(query: String = "", sections: [Section] = [], selection: Row.ID? = nil) {
      (self.query, self.sections, self.selection) = (query, sections, selection)
    }
  }

  /// `Actions`, `Sessions` or `Commands & files`, and its rows best first.
  public struct Section: Identifiable, Equatable, Sendable {
    public var id: String { title }
    public var title: String
    public var rows: [Row]

    public init(title: String, rows: [Row]) { (self.title, self.rows) = (title, rows) }
  }

  public struct Row: Identifiable, Equatable, Sendable {
    public var id: String
    /// An SF Symbol, or a glyph such as `/` drawn as text.
    public var symbol: Symbol
    public var title: String
    /// Offsets of the title's characters the query matched.
    public var matched: [Int]
    /// The keys that run it, or where and when a session ran: `⌘⇧R`, `cox · 2 d ago`.
    public var detail: String

    public init(
      id: String, symbol: Symbol, title: String, matched: [Int] = [], detail: String = ""
    ) {
      (self.id, self.symbol, self.title, self.matched, self.detail) =
        (id, symbol, title, matched, detail)
    }
  }

  public enum Symbol: Equatable, Sendable {
    case system(String)
    case glyph(String)
  }

  public enum Intent: Equatable, Sendable {
    case query(String)
    /// Runs a row: a click, or ⏎ on the selection.
    case pick(Row.ID)
    /// ↓ or ↑ moves the selection by one.
    case move(Int)
    /// Esc, or a click outside.
    case dismiss
  }

  let state: State
  let send: (Intent) -> Void
  @FocusState private var isFocused: Bool

  public init(state: State, send: @escaping (Intent) -> Void) {
    self.state = state
    self.send = send
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.pane, style: .continuous)
    VStack(alignment: .leading, spacing: 0) {
      field
      Hairline()
      VStack(alignment: .leading, spacing: 0) {
        if state.sections.isEmpty {
          Text("No matches")
            .textStyle(.footnote)
            .foregroundStyle(Color(.textSecondary))
            .padding(Space.l)
        }
        ForEach(state.sections) { section in
          SectionHeader(section.title)
            .padding(.horizontal, Space.l)
            .padding(.top, Space.m)
            .padding(.bottom, Space.xs)
          ForEach(section.rows) { row in
            PaletteRow(row: row, isSelected: row.id == state.selection) { send(.pick(row.id)) }
          }
        }
      }
      .padding(Space.s)
    }
    .frame(width: Size.paletteWidth, alignment: .leading)
    .glassPane(shape, surface: Color(.surfacePopover), role: .readable)
    .hairline(in: shape)
    .elevation(.e4, cornerRadius: Radius.pane)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Command palette")
    // Esc wherever focus is inside it; the query field takes focus once it is on screen, after
    // whichever field held it gives it up.
    .onExitCommand { send(.dismiss) }
    .task { isFocused = true }
  }

  private var field: some View {
    HStack(spacing: Space.ml) {
      Image(systemName: "magnifyingglass")
        .symbolStyle(.titlePalette)
        .foregroundStyle(Color(.textSecondary))
        .accessibilityHidden(true)
      TextField(
        "Search actions, sessions, commands and files",
        text: Binding(get: { state.query }, set: { send(.query($0)) })
      )
      .textFieldStyle(.plain)
      .textStyle(.titlePalette)
      .foregroundStyle(Color(.textPrimary))
      .focused($isFocused)
      .onSubmit { if let selection = state.selection { send(.pick(selection)) } }
      .onKeyPress(.downArrow) {
        send(.move(1))
        return .handled
      }
      .onKeyPress(.upArrow) {
        send(.move(-1))
        return .handled
      }
      .onKeyPress(.escape) {
        send(.dismiss)
        return .handled
      }
      KeyCap("esc")
    }
    .padding(.horizontal, Space.xl)
    .padding(.vertical, Space.xl)
  }
}

/// One palette row: its symbol's well, the title with the matched characters bold, and the detail.
private struct PaletteRow: View {
  let row: CommandPalette.Row
  let isSelected: Bool
  let action: () -> Void

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.l, style: .continuous)
    Button(action: action) {
      HStack(spacing: Space.l) {
        icon
        Text(title)
          .textStyle(.transcript)
          .foregroundStyle(isSelected ? Color(.textOnAccent) : Color(.textPrimary))
          .lineLimit(1)
          .truncationMode(.middle)
        Spacer(minLength: Space.m)
        Text(row.detail)
          .textStyle(.footnote)
          .foregroundStyle(isSelected ? Color(.textOnAccent) : Color(.textSecondary))
          .lineLimit(1)
      }
      .padding(.horizontal, Space.l)
      .padding(.vertical, Space.m)
      .background { if isSelected { shape.fill(Color(.accent)) } }
      .contentShape(shape)
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }

  private var icon: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.s, style: .continuous)
    return Group {
      switch row.symbol {
      case .system(let name): Image(systemName: name).symbolStyle(.body)
      case .glyph(let glyph): Text(glyph).textStyle(.body)
      }
    }
    .foregroundStyle(isSelected ? Color(.textOnAccent) : Color(.textSecondary))
    .frame(width: Size.paletteIcon, height: Size.paletteIcon)
    // On the selected row the glyph stands on the accent face itself.
    .background { if !isSelected { shape.fill(Color(.fillSecondary)) } }
    .accessibilityHidden(true)
  }

  /// The title with each matched character bold, as the mockup's `<b>` marks.
  private var title: AttributedString {
    var title = AttributedString(row.title)
    let characters = title.characters
    for offset in row.matched where offset >= 0 && offset < characters.count {
      let start = characters.index(characters.startIndex, offsetBy: offset)
      title[start..<characters.index(after: start)].inlinePresentationIntent = .stronglyEmphasized
    }
    return title
  }
}

extension View {
  /// Mockup 12's overlay while `state` is set: the window dimmed by `shadow.scrim`, which a click
  /// dismisses, under the palette two toolbar heights down.
  public func commandPalette(
    _ state: CommandPalette.State?, send: @escaping (CommandPalette.Intent) -> Void
  ) -> some View {
    overlay(alignment: .top) {
      if let state {
        ZStack(alignment: .top) {
          Color(.shadowScrim)
            .ignoresSafeArea()
            .contentShape(Rectangle())
            .onTapGesture { send(.dismiss) }
          CommandPalette(state: state, send: send)
            .padding(.top, Size.toolbarHeight * 2)
        }
      }
    }
  }
}

extension ShellShortcut {
  /// The palette's key (DT§5.5, T37.44.13); the File menu's command answers it.
  public static let palette = Self(key: .init("k", modifiers: .command), glyphs: "⌘K")
  /// New session, as the sidebar footer's key cap names it; the File menu answers it.
  public static let newSession = Self(key: .init("n", modifiers: .command), glyphs: "⌘N")
}

#Preview("actions") {
  PreviewMatrix { CommandPalette(state: PreviewState.paletteReview) { _ in } }
}
#Preview("empty query") {
  PreviewMatrix { CommandPalette(state: PreviewState.paletteOpen) { _ in } }
}
#Preview("no matches") {
  PreviewMatrix { CommandPalette(state: PreviewState.paletteNoMatch) { _ in } }
}
