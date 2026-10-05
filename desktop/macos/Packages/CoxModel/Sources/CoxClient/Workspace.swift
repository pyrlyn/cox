// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The sessions across projects the sidebar lists (DT§5.1, DT§4.3), field for field as cox-ffi
// exports `cox_app::Workspace`, its sidebar sections and the inbox's per-session activity.
// Separate from the session seam because it answers for every session in `cox.db`, not one
// open handle.

/// `cox_app::Project`: a git root with sessions, newest first.
public struct Project: Equatable, Sendable {
  public var root: String
  /// The root's last path component.
  public var name: String
  public var costUsd: Double

  public init(root: String, name: String, costUsd: Double = 0) {
    (self.root, self.name, self.costUsd) = (root, name, costUsd)
  }
}

/// `cox_app::SessionEntry`: one session row.
public struct SessionEntry: Equatable, Sendable {
  public var id: String
  /// `nil` until the core titled it.
  public var title: String?
  public var cwd: String
  /// RFC 3339, its last write.
  public var updatedAt: String
  public var turns: Int64
  public var costUsd: Double
  /// Another process drives it (T37.34).
  public var isHeld: Bool
  /// The external ACP agent that drove it (T52.6); `nil` for cox.
  public var agent: String?
  /// The best-of-n group it was launched in (T52.9); the sidebar shows a group as one.
  public var bestOf: String?

  public init(
    id: String, title: String? = nil, cwd: String = "", updatedAt: String = "", turns: Int64 = 0,
    costUsd: Double = 0, isHeld: Bool = false, agent: String? = nil,
    bestOf: String? = nil
  ) {
    (self.id, self.title, self.cwd, self.updatedAt) = (id, title, cwd, updatedAt)
    (self.turns, self.costUsd, self.isHeld, self.agent) = (turns, costUsd, isHeld, agent)
    self.bestOf = bestOf
  }
}

/// `cox_app::Activity`: what a session this process drives is doing.
public enum Activity: Equatable, Sendable { case idle, running, waitingOnYou, failed }

/// `cox_app::SidebarStatus`: the status glyph, named as the clients' status dots.
public enum SidebarStatus: Equatable, Sendable { case running, waiting, idle, error }

/// `cox_app::SubtitlePart`. The filter matches text; `age` holds `updated_at` so each client
/// localizes it and the filter never sees that wording.
public enum SubtitlePart: Equatable, Sendable {
  case text(String)
  case age(updatedAt: String)
}

/// `cox_app::SidebarKind`: a status section or a project, as the clients' sidebar group.
public enum SidebarKind: Equatable, Sendable {
  case section(count: String?)
  case project(isExpanded: Bool)
}

/// `cox_app::SidebarRow`: one inbox or session row, before a client joins the subtitle.
public struct SidebarRow: Equatable, Sendable {
  public var id: String
  public var session: String
  public var status: SidebarStatus
  public var title: String
  public var subtitle: [SubtitlePart]
  /// Set only when the session has spent something.
  public var cost: Double?
  public var isReadOnly: Bool

  public init(
    id: String, session: String, status: SidebarStatus, title: String,
    subtitle: [SubtitlePart] = [], cost: Double? = nil, isReadOnly: Bool = false
  ) {
    (self.id, self.session, self.status, self.title) = (id, session, status, title)
    (self.subtitle, self.cost, self.isReadOnly) = (subtitle, cost, isReadOnly)
  }
}

/// `cox_app::SidebarSection`: "Needs you", "Running", or one project.
public struct SidebarSection: Equatable, Sendable {
  public var id: String
  public var title: String
  public var kind: SidebarKind
  public var rows: [SidebarRow]

  public init(id: String, title: String, kind: SidebarKind, rows: [SidebarRow] = []) {
    (self.id, self.title, self.kind, self.rows) = (id, title, kind, rows)
  }
}

/// The workspace half of cox-ffi's `App`.
public protocol WorkspaceClient: Sendable {
  /// Most recently active first, at most `limit`.
  func projects(limit: UInt32) throws -> [Project]
  /// The sessions under `project`'s root, newest first, at most `limit`.
  func sessions(project: String, limit: UInt32) throws -> [SessionEntry]
  /// `.idle` for a session this process has not driven.
  func activity(session: String) -> Activity
  /// "Needs you", "Running", then each project. The core filters and orders; `folded` are
  /// project roots hidden until a filter opens them.
  func sidebar(filter: String, folded: [String]) throws -> [SidebarSection]
  /// Returns once the list may read differently: a commit to `cox.db` from another connection,
  /// or a session here that started, stopped or began to wait.
  func changed() async throws
  /// Sets the title of a session no window here has open, as the person's (A113); an open one
  /// renames through its `Intent.rename`. `false` when the title has no text.
  func rename(session: String, title: String) throws -> Bool
}

/// Fixed projects, sessions and activity: enough to drive the sidebar in a test or preview.
public struct FixtureWorkspace: WorkspaceClient {
  public var fixedProjects: [Project]
  /// By project root.
  public var fixedSessions: [String: [SessionEntry]]
  public var fixedActivity: [String: Activity]
  /// Sections the core would have built; the store still joins subtitle parts.
  public var fixedSidebar: [SidebarSection]

  public init(
    projects: [Project] = [], sessions: [String: [SessionEntry]] = [:],
    activity: [String: Activity] = [:], sidebar: [SidebarSection] = []
  ) {
    (fixedProjects, fixedSessions, fixedActivity, fixedSidebar) = (
      projects, sessions, activity, sidebar
    )
  }

  public func projects(limit: UInt32) -> [Project] { Array(fixedProjects.prefix(Int(limit))) }

  public func sessions(project: String, limit: UInt32) -> [SessionEntry] {
    Array((fixedSessions[project] ?? []).prefix(Int(limit)))
  }

  public func activity(session: String) -> Activity { fixedActivity[session] ?? .idle }

  /// A fixture does not re-filter: the caller already built the sections the core would return.
  public func sidebar(filter: String, folded: [String]) -> [SidebarSection] { fixedSidebar }

  /// A fixed workspace never changes: waits until cancelled.
  public func changed() async throws { try await Task.sleep(for: .seconds(86_400)) }

  /// A fixed workspace keeps its titles.
  public func rename(session: String, title: String) -> Bool { false }
}
