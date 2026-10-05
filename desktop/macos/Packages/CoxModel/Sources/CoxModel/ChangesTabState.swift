// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inspector's Changes tab (T37.29.1, DT§5.1): cox-app's `Changes` as the rows CoxUI's
// `ChangesTab.State` holds — the changed files, the checkpoints with their id and time, and the
// worktree's facts as the core built them (T58.4.22), with the checkpoint time and the worktree's
// size localized here. The app copies them into `ChangesTab.State` field for field.

import CoxClient
import Foundation

public struct ChangesTabState: Equatable, Sendable {
  /// `ChangedFileRow.File`.
  public struct File: Equatable, Sendable {
    public var path: String
    public var change: FileChange
    public var added: Int
    public var removed: Int
  }

  /// `CheckpointRow.Checkpoint`; `id` is the turn a rewind goes back to.
  public struct Checkpoint: Equatable, Sendable {
    public var id: String
    public var label: String
    public var time: String
  }

  /// `KeyValueGrid.Row`.
  public struct Fact: Equatable, Sendable {
    public var label: String
    public var values: [String]
  }

  public var files: [File] = []
  public var checkpoints: [Checkpoint] = []
  /// Branch, base and size; empty outside a linked worktree.
  public var worktree: [Fact] = []

  public init() {}

  /// `locale` and `timeZone` format a checkpoint's time and the worktree's size.
  public init(_ changes: Changes, locale: Locale = .current, timeZone: TimeZone = .current) {
    files = changes.files.map(File.init)
    let clock = Date.FormatStyle(
      date: .omitted, time: .shortened, locale: locale, timeZone: timeZone)
    checkpoints = changes.checkpoints.map {
      Checkpoint(
        id: String($0.turn), label: $0.label,
        time: Self.date($0.time).map { $0.formatted(clock) } ?? "")
    }
    guard let tree = changes.worktree else { return }
    worktree = changes.worktreeFacts.map {
      Fact(label: $0.label, values: $0.value.map { [$0] } ?? [])
    }
    let size = ByteCountFormatStyle(style: .file, locale: locale)
    worktree.append(Fact(label: "Size", values: [Int64(clamping: tree.bytes).formatted(size)]))
  }

  /// An RFC 3339 time as cox.db writes it, with or without milliseconds; the sidebar's too.
  static func date(_ text: String) -> Date? {
    let withFraction = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
    return (try? withFraction.parse(text)) ?? (try? Date.ISO8601FormatStyle().parse(text))
  }
}

extension ChangesTabState.File {
  /// A changed file as a row; Review's turns list the same rows.
  init(_ file: ChangedFile) {
    self.init(
      path: file.path, change: file.change, added: Int(file.added), removed: Int(file.removed))
  }
}

extension SessionStore {
  /// The Changes tab's state, read from the core when the tab asks (T37.29.1).
  public func changesTab() async throws -> ChangesTabState {
    ChangesTabState(try await session.changes())
  }
}
