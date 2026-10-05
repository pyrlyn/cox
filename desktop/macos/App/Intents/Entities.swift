// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The App Intents entities (T51.17): a project and a session as Shortcuts and Siri pick them,
// read from the workspace the sidebar reads (`App.projects()`, `App.sessions()`). Titles come
// from Rust already sanitized. Separate from the intents so both intents share one query each.

import AppIntents
import CoxClient

/// A project, by its root.
struct ProjectEntity: AppEntity {
  static let typeDisplayRepresentation: TypeDisplayRepresentation = "Project"
  static var defaultQuery: ProjectQuery { ProjectQuery() }

  let id: String
  let name: String

  var displayRepresentation: DisplayRepresentation {
    DisplayRepresentation(title: "\(name)", subtitle: "\(id)")
  }
}

struct ProjectQuery: EntityQuery {
  @Dependency private var model: AppModel

  @MainActor
  func entities(for identifiers: [ProjectEntity.ID]) async throws -> [ProjectEntity] {
    try projects().filter { identifiers.contains($0.id) }
  }

  @MainActor
  func suggestedEntities() async throws -> [ProjectEntity] { try projects() }

  @MainActor
  private func projects() throws -> [ProjectEntity] {
    try model.launch.live.get().projects(limit: Listed.projectLimit).map {
      ProjectEntity(id: $0.root, name: $0.name)
    }
  }
}

/// A session, by its id.
struct SessionEntity: AppEntity {
  static let typeDisplayRepresentation: TypeDisplayRepresentation = "Session"
  static var defaultQuery: SessionQuery { SessionQuery() }

  let id: String
  let title: String
  let project: String

  var displayRepresentation: DisplayRepresentation {
    DisplayRepresentation(title: "\(title)", subtitle: "\(project)")
  }
}

struct SessionQuery: EntityQuery {
  @Dependency private var model: AppModel

  @MainActor
  func entities(for identifiers: [SessionEntity.ID]) async throws -> [SessionEntity] {
    try sessions().filter { identifiers.contains($0.id) }
  }

  @MainActor
  func suggestedEntities() async throws -> [SessionEntity] { try sessions() }

  /// Each recent project's recent sessions, newest first within a project.
  @MainActor
  private func sessions() throws -> [SessionEntity] {
    let workspace = try model.launch.live.get()
    return try workspace.projects(limit: Listed.projectLimit).flatMap { project in
      try workspace.sessions(project: project.root, limit: Listed.sessionLimit).map {
        SessionEntity(id: $0.id, title: $0.title ?? "Untitled", project: project.name)
      }
    }
  }
}

/// How much of the workspace a picker lists.
private enum Listed {
  static let projectLimit: UInt32 = 20
  static let sessionLimit: UInt32 = 20
}
