// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The menu-bar extra's rows (T51.14, mockup 26, DT§5.6): the inbox's approvals and questions
// that can still be answered, each under its session's title, the sessions running now as the
// sidebar lists them, and today's figures from Rust's ledger summary. Here, not in CoxUI,
// because these decide what the panel shows (DS§1); the app copies them into CoxUI's
// `MenuBarPanel.State` field for field. Allow and Deny go through the notifications' route.

import CoxClient

public struct MenuBarState: Equatable, Sendable {
  /// What a `Need` asks for; one level up so the types nest only one deep.
  public enum NeedKind: Equatable, Sendable {
    /// What the call acts on: the command, the path.
    case approval(command: String)
    case question(String)
  }

  public struct Need: Identifiable, Equatable, Sendable {
    /// The inbox item's id, as `InboxRow`'s.
    public let id: String
    public let session: String
    /// The approval or question Allow, Deny or an answer names.
    public let call: String
    /// The session's title.
    public let title: String
    public let kind: NeedKind
  }

  public struct Running: Identifiable, Equatable, Sendable {
    /// The session's id.
    public let id: String
    public let title: String
    /// The sidebar row's line: `cox · running`.
    public let activity: String
    public let cost: String
  }

  public let needs: [Need]
  public let running: [Running]
  /// `$4.02 · 7 sessions`; empty until read.
  public let today: String

  /// `title` names a session by its id; `running` is the sidebar's "Running" rows.
  public init(
    inbox: [InboxItem], running: [SidebarRow], today: String, title: (String) -> String?
  ) {
    needs = inbox.compactMap { item in
      guard !item.expired else { return nil }
      let (call, kind): (String, NeedKind)
      switch item.need {
      case .approval(let id, let tool, let subject, _):
        (call, kind) = (id, .approval(command: subject.isEmpty ? tool : subject))
      case .question(let id, let question, _): (call, kind) = (id, .question(question))
      case .failed, .taskDone: return nil
      }
      return Need(
        id: "\(item.session)#\(item.seq)", session: item.session, call: call,
        title: title(item.session) ?? "Untitled session", kind: kind)
    }
    self.running = running.map {
      Running(id: $0.session, title: $0.title, activity: $0.subtitle, cost: $0.cost ?? "")
    }
    self.today = today
  }
}

extension SettingsStore {
  /// `desktop.menu_bar`: the extra shows unless the person turned it off (T51.14).
  public var showsMenuBar: Bool {
    view.flatMap { SectionRows($0.settings, "desktop").decode("menu_bar") } ?? true
  }
}
