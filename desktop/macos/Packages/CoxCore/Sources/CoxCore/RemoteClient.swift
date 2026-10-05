// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A remote host over cox-ffi (DT§4.4, T52.20, T52.21): `App.connectRemote` as a
// `RemoteWorkspace`, and a remote session as a `SessionClient`, so the stores and the windows
// show it as they show a local one. Separate from `LiveCoreClient.swift` because the wire answers
// only what `cox app-server` serves: what it has no message for says so instead of guessing.

import CoxClient
import CoxFFIBindings
import Foundation

extension LiveCoreClient: RemoteConnector {
  public func connect(host: String) async throws -> any RemoteWorkspace {
    LiveRemote(try await app.connectRemote(host: host))
  }
}

final class LiveRemote: RemoteWorkspace {
  private let handle: RemoteHandle

  init(_ handle: RemoteHandle) { self.handle = handle }

  var host: String { handle.host() }
  var isConnected: Bool { handle.isConnected() }

  func reconnect() async throws { try await handle.reconnect() }

  func projects(limit: UInt32) async throws -> [CoxClient.Project] {
    try await handle.projects(limit: limit).map { CoxClient.Project($0) }
  }

  func sessions(project: String, limit: UInt32) async throws -> [CoxClient.SessionEntry] {
    try await handle.sessions(project: project, limit: limit).map { CoxClient.SessionEntry($0) }
  }

  func open(_ request: OpenSession) async throws -> any SessionClient {
    RemoteSession(
      try await handle.open(
        // The remote wire opens cox sessions only (T52.7): an agent runs where it is installed.
        request: OpenRequest(
          cwd: request.cwd, resume: request.resume, theme: request.theme, agent: nil)))
  }
}

/// A session on a remote host. Its first frame is the blocks it opened with; its patches stream
/// as a local session's do. Calls the stores make synchronously and the wire answers only
/// asynchronously come back empty or refused.
final class RemoteSession: SessionClient {
  private let handle: RemoteSessionHandle

  init(_ handle: RemoteSessionHandle) { self.handle = handle }

  var id: String { handle.id() }

  func snapshot() -> [CoxClient.Block] { handle.opened().map { CoxClient.Block($0) } }

  func nextPatches() async -> [CoxClient.TimelinePatch]? {
    await handle.nextPatches()?.map { CoxClient.TimelinePatch($0) }
  }

  func send(_ intent: CoxClient.Intent) async throws -> (any SessionClient)? {
    try await handle.send(intent: CoxFFIBindings.Intent(intent)).map { RemoteSession($0) }
  }

  /// The wire's completion is a round trip; the composer asks on each keystroke and waits for
  /// none.
  func complete(_ token: String, limit: UInt32) -> [CoxClient.Completion] { [] }

  func palette(
    _ query: String, items: [CoxClient.PaletteItem], limit: UInt32
  ) -> [CoxClient.PaletteHit] {
    CoxClient.PaletteHit.ranked(query, items, limit: limit)
  }

  func typedToken(
    _ text: String, caret: Int, selection: Bool, shell: Bool
  ) -> CoxClient.TypedToken? {
    ComposerRules.typedToken(text, caret: caret, selection: selection, shell: shell)
  }

  func pick(_ text: String, token: CoxClient.TypedToken, insert: String) -> CoxClient.Splice? {
    ComposerRules.pick(text, token: token, insert: insert)
  }

  func mentions(_ text: String, picked: [String]) -> [String] {
    CoxFFIBindings.mentions(text: text, picked: picked)
  }

  func draftIntent(
    _ text: String, shell: Bool, attachments: Int, running: Bool, when: CoxClient.SendWhen
  ) -> CoxClient.DraftIntent {
    ComposerRules.draftIntent(
      text, shell: shell, attachments: attachments, running: running, when: when)
  }

  func history(limit: UInt32) throws -> [String] { [] }

  func changes() async throws -> CoxClient.Changes {
    CoxClient.Changes(try await handle.changes())
  }

  func review(_ path: String) async throws -> CoxClient.DiffModel? { nil }

  func reviewMessage(_ comments: [CoxClient.LineComment]) -> String? {
    CoxFFIBindings.reviewMessage(comments: comments.map { CoxFFIBindings.LineComment($0) })
  }

  func plan() -> [CoxClient.TodoItem] { [] }

  func openTask(_ task: String) throws -> CoxClient.TaskTarget? { nil }

  func output(archive: String) throws -> String { throw RemoteUnsupported("Expanding an output") }

  func info() async throws -> CoxClient.Info { throw RemoteUnsupported("Info") }

  func turnCosts() async throws -> CoxClient.TurnCosts { throw RemoteUnsupported("Turn costs") }

  /// A shell on the host would need a PTY over the wire; the terminal pane is local only.
  func openTerminal(cols: UInt16, rows: UInt16) throws -> any TerminalClient {
    throw RemoteUnsupported("A terminal")
  }

  /// Stops the stream; the host keeps the session and any running turn.
  func close() {
    let handle = handle
    Task { try? await handle.close() }
  }
}
