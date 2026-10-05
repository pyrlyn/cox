// A prompt's rewind menu (Figma frame 14-rewind-edit-resend): the turn it rewinds to before, how
// many files a code rewind restores, and the rewind or fork the menu picks. The rewind and the
// fork are the core's (`Intent.rewind`, `Intent.fork`); the count is read from the session's
// `Changes` (T37.46). `RewindPreviewService` stays the seam so a test or preview can give any
// count. The app copies the state into CoxUI's `RewindMenu.State`.

import CoxClient

/// How many files a code rewind to before a turn restores.
public protocol RewindPreviewService: Sendable {
  func restoredFiles(beforeTurn turn: UInt32) async throws -> Int
}

/// The count from the core's Changes tab (T37.46): `Changes.turns` lists each changed file once,
/// under the turn that changed it last, so the files under turn N or later are exactly those a
/// code rewind to before N restores. A shell command's writes are not listed, as in the tab.
public struct ChangesRewindPreview: RewindPreviewService {
  let session: any SessionClient

  public init(session: any SessionClient) { self.session = session }

  public func restoredFiles(beforeTurn turn: UInt32) async throws -> Int {
    Self.restored(try await session.changes(), beforeTurn: turn)
  }

  static func restored(_ changes: Changes, beforeTurn turn: UInt32) -> Int {
    changes.turns.filter { $0.turn >= turn }.reduce(0) { $0 + $1.files.count }
  }
}

public struct RewindMenuState: Equatable, Sendable {
  public var turn: UInt32
  /// `nil` until the preview answers.
  public var restoredFiles: Int?

  public init(turn: UInt32, restoredFiles: Int? = nil) {
    (self.turn, self.restoredFiles) = (turn, restoredFiles)
  }
}

extension SessionStore {
  /// The menu for a prompt's turn, with the count `preview` gives (the session's own changes
  /// when none is given); a failed preview leaves the count unknown rather than the menu closed.
  public func rewindMenu(
    turn: UInt32, preview: (any RewindPreviewService)? = nil
  ) async -> RewindMenuState {
    let preview = preview ?? ChangesRewindPreview(session: session)
    return RewindMenuState(
      turn: turn, restoredFiles: try? await preview.restoredFiles(beforeTurn: turn))
  }

  /// Fork a new session here: a child that starts from the conversation before `turn`.
  @discardableResult
  public func fork(beforeTurn turn: UInt32) async throws -> (any SessionClient)? {
    try await send(.fork(turn: turn))
  }
}
