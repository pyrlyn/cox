// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The sidebar's workspace over cox-ffi (DT§5.1, DT§4.3): `App.projects`, `sessions`,
// `activity`, `sidebar` and the change wait as CoxClient's values, the toolbar's model catalog
// and menu (T58.4.12) and the footer's usable providers (T37.22.6, A110), the launch's
// login-shell environment (DT§4.8) and the browser pane's address check (T51.10), and the row
// conversions a remote host's list shares (T52.21). Separate from `LiveCoreClient.swift` like
// the other conversions, so that file stays the list of calls into Rust for one session.

import CoxClient
import CoxFFIBindings

extension LiveCoreClient: WorkspaceClient {
  public func projects(limit: UInt32) throws -> [CoxClient.Project] {
    try app.projects(limit: limit).map { CoxClient.Project($0) }
  }

  public func sessions(project: String, limit: UInt32) throws -> [CoxClient.SessionEntry] {
    try app.sessions(project: project, limit: limit).map { CoxClient.SessionEntry($0) }
  }

  public func activity(session: String) -> CoxClient.Activity {
    switch app.activity(session: session) {
    case .idle: .idle
    case .running: .running
    case .waitingOnYou: .waitingOnYou
    case .failed: .failed
    }
  }

  public func sidebar(filter: String, folded: [String]) throws -> [CoxClient.SidebarSection] {
    try app.sidebar(filter: filter, folded: folded).map { CoxClient.SidebarSection($0) }
  }

  public func changed() async throws { try await app.workspaceChanged() }

  /// The menu bar's "Today" figures, `$4.02 · 7 sessions`, from the cost ledger (T51.12).
  public func today() throws -> String { try app.today().text }

  public func rename(session: String, title: String) throws -> Bool {
    try app.rename(session: session, title: title)
  }

  /// Reads the login shell's environment into this process; call once at launch, before a
  /// session opens. Returns why it kept the inherited environment, if it did.
  public static func loadLoginEnv() async throws -> String? {
    try await CoxFFIBindings.loadLoginEnv()
  }

  /// The browser pane's typed address as the URL to load, or `nil` when it is not an `http` or
  /// `https` page: Rust's check, the one `browser_open` makes (T51.10).
  public static func webAddress(_ text: String) -> String? {
    CoxFFIBindings.webAddress(text: text)
  }
}

// The local and the remote workspace list the same rows (T52.21).
extension CoxClient.Project {
  init(_ value: CoxFFIBindings.Project) {
    self.init(root: value.root, name: value.name, costUsd: value.costUsd)
  }
}

extension CoxClient.SessionEntry {
  init(_ value: CoxFFIBindings.SessionEntry) {
    self.init(
      id: value.info.id, title: value.info.title, cwd: value.info.cwd,
      updatedAt: value.info.updatedAt, turns: value.info.turns, costUsd: value.info.costUsd,
      isHeld: value.heldBy != nil, agent: value.agent, bestOf: value.bestOf)
  }
}

extension CoxClient.SidebarSection {
  init(_ value: CoxFFIBindings.SidebarSection) {
    self.init(
      id: value.id, title: value.title, kind: .init(value.kind),
      rows: value.rows.map { CoxClient.SidebarRow($0) })
  }
}

extension CoxClient.SidebarRow {
  init(_ value: CoxFFIBindings.SidebarRow) {
    self.init(
      id: value.id, session: value.session, status: .init(value.status), title: value.title,
      subtitle: value.subtitle.map { CoxClient.SubtitlePart($0) }, cost: value.cost,
      isReadOnly: value.isReadOnly)
  }
}

extension CoxClient.SidebarKind {
  init(_ value: CoxFFIBindings.SidebarKind) {
    switch value {
    case .section(let count): self = .section(count: count)
    case .project(let isExpanded): self = .project(isExpanded: isExpanded)
    }
  }
}

extension CoxClient.SidebarStatus {
  init(_ value: CoxFFIBindings.SidebarStatus) {
    switch value {
    case .running: self = .running
    case .waiting: self = .waiting
    case .idle: self = .idle
    case .error: self = .error
    }
  }
}

extension CoxClient.SubtitlePart {
  init(_ value: CoxFFIBindings.SubtitlePart) {
    switch value {
    case .text(let text): self = .text(text)
    case .age(let updatedAt): self = .age(updatedAt: updatedAt)
    }
  }
}

extension LiveCoreClient: AgentsClient {
  public func agents(cwd: String) async throws -> [CoxClient.AgentChoice] {
    try await app.agents(cwd: cwd).map {
      CoxClient.AgentChoice(
        name: $0.name, label: $0.label, origin: $0.origin, launch: $0.launch,
        unavailable: $0.unavailable)
    }
  }
}

extension LiveCoreClient: ModelsClient {
  public func models(cwd: String) throws -> [CoxClient.ModelChoice] {
    try app.models(cwd: cwd).map {
      CoxClient.ModelChoice(
        tier: CoxClient.Tier($0.tier), provider: $0.provider, id: $0.id,
        displayName: $0.displayName, shortName: $0.shortName,
        efforts: $0.efforts.map { CoxClient.Effort($0) },
        contextWindow: $0.contextWindow)
    }
  }

  public func modelMenu(cwd: String) throws -> [CoxClient.ModelSection] {
    try app.modelMenu(cwd: cwd).map { section in
      CoxClient.ModelSection(
        tier: CoxClient.Tier(section.tier), title: section.title,
        models: section.models.map {
          CoxClient.MenuModel(
            id: $0.id, displayName: $0.displayName, shortName: $0.shortName,
            efforts: $0.efforts)
        })
    }
  }

  public func usableProviders(cwd: String) async throws -> [String] {
    try await app.usableProviders(cwd: cwd)
  }
}
