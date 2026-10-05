// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// ⌘K's command palette in a session window (DT§5.5, mockup 12, T37.44.13): the window's actions
// and the listed sessions go to the session, which ranks them with its `/` commands and `@` files
// through `cox_app::palette::rank`; a picked row runs its action, shows its session or lands in
// the composer. Separate from `SessionWindow.swift` to keep that file within SwiftLint's length
// limits; nothing here ranks or filters.

import CoxClient
import CoxModel
import CoxUI
import SwiftUI

/// The palette while it shows: the query, what the session ranked for it and the row ⏎ runs.
struct SessionPalette: Equatable {
  var query = ""
  var hits: [PaletteHit] = []
  var selection: String?

  /// At most this many rows of each kind.
  static let limit: UInt32 = 5

  /// The rows as mockup 12 groups them: actions, sessions, then commands and files together.
  var state: CommandPalette.State {
    let groups: [(String, Set<PaletteKind>)] = [
      ("Actions", [.action]), ("Sessions", [.session]), ("Commands & files", [.command, .file]),
    ]
    let sections = groups.compactMap { title, kinds -> CommandPalette.Section? in
      let rows = hits.filter { kinds.contains($0.item.kind) }.map(Self.row)
      return rows.isEmpty ? nil : CommandPalette.Section(title: title, rows: rows)
    }
    return CommandPalette.State(query: query, sections: sections, selection: selection)
  }

  /// ↓ or ↑: the next or previous row, stopping at either end.
  mutating func move(_ step: Int) {
    let ids = hits.map(\.item.id)
    guard let index = selection.flatMap(ids.firstIndex(of:)) else { return }
    selection = ids[min(max(index + step, 0), ids.count - 1)]
  }

  private static func row(_ hit: PaletteHit) -> CommandPalette.Row {
    let symbol: CommandPalette.Symbol =
      switch hit.item.kind {
      case .action: .system(PaletteAction(rawValue: hit.item.id)?.symbol ?? "command")
      case .session: .system("bubble.left")
      case .command: .glyph("/")
      case .file: .system("doc")
      }
    return CommandPalette.Row(
      id: hit.item.id, symbol: symbol, title: hit.item.title, matched: hit.matched,
      detail: hit.item.detail)
  }
}

/// What the palette offers besides sessions, commands and files.
enum PaletteAction: String, CaseIterable {
  case newSession = "action.new-session"
  case review = "action.review"
  case rewind = "action.rewind"
  case revert = "action.revert"
  case sidebar = "action.sidebar"
  case inspector = "action.inspector"
  case terminal = "action.terminal"
  case browser = "action.browser"
  case connectHost = "action.connect-host"

  var symbol: String {
    switch self {
    case .newSession: "plus"
    case .review: "eye"
    case .rewind: "arrow.uturn.backward"
    case .revert: "arrow.clockwise"
    case .sidebar: "sidebar.left"
    case .inspector: "sidebar.right"
    case .terminal: "terminal"
    case .browser: "globe"
    case .connectHost: "network"
    }
  }

  /// This action's row, titled `title`, with the keys that run it.
  func item(_ title: String, _ keys: ShellShortcut? = nil) -> PaletteItem {
    PaletteItem(kind: .action, id: rawValue, title: title, detail: keys?.glyphs ?? "")
  }
}

extension SessionWindow {
  /// ⌘K: opens the palette with nothing typed, or closes it.
  func togglePalette() {
    if palette == nil { rank("") } else { palette = nil }
  }

  func handle(_ intent: CommandPalette.Intent) {
    switch intent {
    case .query(let query): rank(query)
    case .move(let step): palette?.move(step)
    case .dismiss: palette = nil
    case .pick(let id):
      guard let hit = palette?.hits.first(where: { $0.item.id == id }) else { return }
      palette = nil
      run(hit.item)
    }
  }

  /// ⌘⇧R: shows Review in the column, or hides it again.
  func toggleReview() {
    reviewing = reviewing == nil ? Reviewing(path: nil) : nil
  }

  /// Ranks for `query`, keeping the selected row while it still shows.
  private func rank(_ query: String) {
    guard let session = showing?.store.session else { return }
    let items = actions + model.sidebar.paletteItems
    let hits = session.palette(query, items: items, limit: SessionPalette.limit)
    let kept = palette?.selection.flatMap { id in hits.contains { $0.item.id == id } ? id : nil }
    palette = SessionPalette(query: query, hits: hits, selection: kept ?? hits.first?.item.id)
  }

  /// The actions this window can run now, titled for the state they change.
  private var actions: [PaletteItem] {
    typealias Action = PaletteAction
    var items = [
      Action.newSession.item("New session", .newSession),
      Action.review.item(reviewing == nil ? "Review changes" : "Hide Review", .review),
      Action.rewind.item("Rewind to turn…"),
      Action.revert.item("Revert file to checkpoint…"),
    ]
    if popOut == nil {
      items.append(Action.sidebar.item(screen.isSidebarVisible ? "Hide Sidebar" : "Show Sidebar"))
    }
    items += [
      Action.inspector.item(screen.isInspectorVisible ? "Hide Inspector" : "Show Inspector"),
      Action.terminal.item(isTerminalShown ? "Hide Terminal" : "Show Terminal", .terminal),
      Action.browser.item(isBrowserVisible ? "Hide Browser" : "Show Browser", .browser),
    ]
    if model.remotes.canConnect { items.append(Action.connectHost.item("Connect to Host…")) }
    return items
  }

  private func run(_ item: PaletteItem) {
    switch item.kind {
    case .session: handle(Sidebar.Intent.select(item.id))
    case .command, .file: showing?.composer.append(item.id)
    case .action: if let action = PaletteAction(rawValue: item.id) { run(action) }
    }
  }

  private func run(_ action: PaletteAction) {
    switch action {
    case .newSession: Task { await newSession() }
    case .review: toggleReview()
    // Checkpoints, and rewinding or reverting to one, live in the Changes tab.
    case .rewind, .revert: (screen.inspectorTab, screen.isInspectorVisible) = (.changes, true)
    case .sidebar: toggleSidebar()
    case .inspector: screen.isInspectorVisible.toggle()
    case .terminal: toggleTerminal()
    case .browser: isBrowserVisible.toggle()
    case .connectHost: connecting = ConnectHostSheet.State()
    }
  }
}
