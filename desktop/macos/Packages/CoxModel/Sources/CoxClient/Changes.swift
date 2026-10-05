// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the inspector's Changes tab lists (T37.29.1, DT§5.1), field for field as cox-ffi exports
// `cox_app::Changes`: the files the session changed, the turns code can be rewound to, and the
// linked worktree it runs in. Separate from the timeline because it answers a call, not a patch.

public struct Changes: Equatable, Sendable {
  /// In the order they were first changed.
  public var files: [ChangedFile]
  /// Oldest first.
  public var checkpoints: [Checkpoint]
  /// `nil` when the session runs outside a linked worktree.
  public var worktree: Linked?
  /// The worktree's branch and base (T58.4.20); its size is each client's to format.
  public var worktreeFacts: [Fact]
  /// `files` grouped by the turn that changed each last, oldest turn first.
  public var turns: [TurnFiles]

  public init(
    files: [ChangedFile] = [], checkpoints: [Checkpoint] = [], worktree: Linked? = nil,
    worktreeFacts: [Fact] = [], turns: [TurnFiles] = []
  ) {
    (self.files, self.checkpoints, self.worktree) = (files, checkpoints, worktree)
    (self.worktreeFacts, self.turns) = (worktreeFacts, turns)
  }
}

/// The files one turn changed last (`cox_app::TurnFiles`).
public struct TurnFiles: Equatable, Sendable {
  public var turn: UInt32
  public var files: [ChangedFile]

  public init(turn: UInt32, files: [ChangedFile]) {
    (self.turn, self.files) = (turn, files)
  }
}

/// How the session left a file.
public enum FileChange: Equatable, Sendable {
  case edited, created, deleted
}

public struct ChangedFile: Equatable, Sendable {
  /// Relative to the session's cwd when inside it.
  public var path: String
  public var change: FileChange
  public var added: UInt32
  public var removed: UInt32
  /// The last call that changed it, and that call's turn.
  public var call: String
  public var turn: UInt32

  public init(
    path: String, change: FileChange, added: UInt32, removed: UInt32, call: String, turn: UInt32
  ) {
    (self.path, self.change, self.added, self.removed) = (path, change, added, removed)
    (self.call, self.turn) = (call, turn)
  }
}

/// A turn that changed files; rewinding code to it (`Intent.rewind`'s `toTurn`) restores them.
public struct Checkpoint: Equatable, Sendable {
  public var turn: UInt32
  /// `Turn 2 · before retry.rs and 1 more`.
  public var label: String
  /// RFC 3339: when the turn started.
  public var time: String

  public init(turn: UInt32, label: String, time: String) {
    (self.turn, self.label, self.time) = (turn, label, time)
  }
}

/// The linked worktree a session runs in (`cox_tools::git::Linked`).
public struct Linked: Equatable, Sendable {
  public var path: String
  /// `nil` when detached.
  public var branch: String?
  /// What the branch is compared with: `origin/<default>`, else the main checkout's branch.
  public var base: String?
  /// The short merge-base of `HEAD` and `base`.
  public var commit: String?
  public var bytes: UInt64

  public init(
    path: String, branch: String? = nil, base: String? = nil, commit: String? = nil, bytes: UInt64
  ) {
    (self.path, self.branch, self.base, self.commit, self.bytes) = (
      path, branch, base, commit, bytes
    )
  }
}

/// One of Review's line comments, field for field as cox-ffi exports
/// `cox_app::review::LineComment` (T37.28.4).
public struct LineComment: Equatable, Sendable {
  public var path: String
  /// Numbers the file before the session changed it when `removed`, else the file on disk.
  public var line: UInt32
  /// The diff shows the line as deleted.
  public var removed: Bool
  public var text: String

  public init(path: String, line: UInt32, removed: Bool = false, text: String) {
    (self.path, self.line, self.removed, self.text) = (path, line, removed, text)
  }
}
