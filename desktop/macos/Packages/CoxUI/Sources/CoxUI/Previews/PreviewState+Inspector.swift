// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the inspector's rows (T37.21.9) and tabs (T37.29): the Changes
// tab's files, checkpoints and worktree as the mockup's inspector shows them. Separate from `PreviewState.swift` so
// molecules built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// The session's changed files; the first is the open one.
  static let changedFiles: [ChangedFileRow.File] = [
    .init(path: "cox-provider-http/src/retry.rs", change: .edited, added: 18, removed: 4),
    .init(path: "cox-provider-http/Cargo.toml", change: .edited, added: 1, removed: 0),
    .init(path: "cox-provider-http/tests/backoff.rs", change: .created, added: 42, removed: 0),
  ]

  /// A file the session removed (T37.29.7).
  static let deletedFile = ChangedFileRow.File(
    path: "cox-provider-http/src/backoff_old.rs", change: .deleted, added: 0, removed: 27)

  static let checkpoints: [CheckpointRow.Checkpoint] = [
    .init(id: "cp-1", label: "Turn 1 · before edit retry.rs", time: "14:02"),
    .init(id: "cp-2", label: "Turn 1 · before write backoff.rs", time: "14:03"),
  ]

  /// The mockup's worktree facts.
  static let worktree: [KeyValueGrid.Row] = [
    .init(label: "Branch", values: ["wt/retry-jitter"]),
    .init(label: "Base", values: ["main @ 4273daa"]),
    .init(label: "Size", values: ["412 MB"]),
    .init(label: "target/", values: ["398 MB"], isDetail: true),
  ]

  /// The mockup's Changes tab, with the first file open in Review.
  static let changes = ChangesTab.State(
    files: changedFiles, selection: changedFiles[0].path, checkpoints: checkpoints,
    worktree: worktree)

  /// What you can do to a changed file.
  static let fileActions = ChangesTab.fileActions(changedFiles[0].path) { _ in }

  static let checkpointActions = ChangesTab.checkpointActions(checkpoints[0].id) { _ in }
}

/// The first changed file at the inspector's width, selected or not.
struct ChangedFileSample: View {
  let isSelected: Bool

  var body: some View {
    ChangedFileRow(
      PreviewState.changedFiles[0], isSelected: isSelected, actions: PreviewState.fileActions
    )
    .frame(width: Size.inspectorWidth)
  }
}

/// Every changed file stacked at the inspector's width, none selected.
struct ChangedFileList: View {
  var body: some View {
    VStack(spacing: 0) {
      ForEach(PreviewState.changedFiles, id: \.path) {
        ChangedFileRow($0, actions: PreviewState.fileActions)
      }
    }
    .frame(width: Size.inspectorWidth)
  }
}

/// A deleted file at the inspector's width.
struct DeletedFileSample: View {
  var body: some View {
    ChangedFileRow(PreviewState.deletedFile, actions: PreviewState.fileActions)
      .frame(width: Size.inspectorWidth)
  }
}

/// The first checkpoint at the inspector's width, selected or not.
struct CheckpointSample: View {
  let isSelected: Bool

  var body: some View {
    CheckpointRow(
      PreviewState.checkpoints[0], isSelected: isSelected,
      actions: PreviewState.checkpointActions
    )
    .frame(width: Size.inspectorWidth)
  }
}

/// Every checkpoint stacked at the inspector's width, none selected.
struct CheckpointList: View {
  var body: some View {
    VStack(spacing: 0) {
      ForEach(PreviewState.checkpoints, id: \.id) {
        CheckpointRow($0, actions: PreviewState.checkpointActions)
      }
    }
    .frame(width: Size.inspectorWidth)
  }
}

/// The inspector on its Changes tab, as tall as the smallest window.
struct ChangesInspectorSample: View {
  let state: ChangesTab.State

  var body: some View {
    Inspector(selection: .changes, content: ChangesTab(state: state) { _ in }) { _ in }
      .frame(height: Size.windowMinHeight)
  }
}
