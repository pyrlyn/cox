// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A session's terminal tabs (T51.6, DT§3.2, mockup 24): the shells `+` opened, which one the
// pane shows, and closing them with the session. Separate from the timeline because nothing a
// shell prints is a patch; the shells run in Rust (`cox_app::terminal`) and this only keeps
// their handles.

import CoxClient

/// One terminal tab: the shell's handle under an id no later tab reuses.
public struct TerminalTab: Identifiable, Sendable {
  public let id: Int
  public let client: any TerminalClient

  /// The tab's title, `zsh — wt/retry-jitter`: the shell's name, then the branch of the
  /// session's linked worktree when it has one.
  public static func title(shell: String, branch: String?) -> String {
    let name = shell.split(separator: "/").last.map(String.init) ?? shell
    return branch.map { "\(name) — \($0)" } ?? name
  }
}

extension SessionStore {
  /// The size a shell starts at, until its view reports the cells it has.
  static let terminalCols: UInt16 = 80
  static let terminalRows: UInt16 = 24

  /// Opens another shell of this session and shows its tab.
  @discardableResult
  public func openTerminal() throws -> TerminalTab {
    let client = try session.openTerminal(cols: Self.terminalCols, rows: Self.terminalRows)
    terminalCount += 1
    let tab = TerminalTab(id: terminalCount, client: client)
    terminals.append(tab)
    terminalSelection = tab.id
    return tab
  }

  /// Closes one tab's shell; the pane then shows the tab before it, or the next.
  public func closeTerminal(_ id: TerminalTab.ID) {
    guard let index = terminals.firstIndex(where: { $0.id == id }) else { return }
    terminals.remove(at: index).client.close()
    guard terminalSelection == id else { return }
    terminalSelection = terminals.isEmpty ? nil : terminals[max(index - 1, 0)].id
  }

  /// Closes every shell: the session's window is closing.
  public func closeTerminals() {
    for tab in terminals { tab.client.close() }
    terminals = []
    terminalSelection = nil
  }

  /// A tab runs a foreground job, which closing the window would kill.
  public var hasBusyTerminal: Bool { terminals.contains { $0.client.isBusy() } }
}
