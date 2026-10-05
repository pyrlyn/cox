// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ChangesTab` (DS§6.4 row `ChangesTab`, the mockup's Changes `.ib`; DT§5.1 Changes): the
// inspector's first tab — the files this session changed, the checkpoints it can rewind to and
// the worktree it runs in. Separate so the `Inspector` frame stays a slot and each tab is its
// own view, fed plain values the app copies from the core (T37.29).

import SwiftUI

/// Up to three `InspectorSection`s: the changed files under a Review button, the checkpoints,
/// the worktree's facts. A section with nothing to list is left out; with none, one quiet line.
public struct ChangesTab: View {
  /// What the tab lists, formatted by the core.
  public struct State: Equatable, Sendable {
    public var files: [ChangedFileRow.File] = []
    /// The path of the file Review has open; its row is lifted.
    public var selection: String?
    public var checkpoints: [CheckpointRow.Checkpoint] = []
    /// Branch, base and size; empty when the session runs outside a worktree.
    public var worktree: [KeyValueGrid.Row] = []

    public init(
      files: [ChangedFileRow.File] = [], selection: String? = nil,
      checkpoints: [CheckpointRow.Checkpoint] = [], worktree: [KeyValueGrid.Row] = []
    ) {
      (self.files, self.selection, self.checkpoints, self.worktree) = (
        files, selection, checkpoints, worktree
      )
    }
  }

  /// What the tab asks the app to do.
  public enum Intent: Equatable, Sendable {
    /// Open Review on the session's changes (DT§5.4).
    case review
    /// Open Review at this file.
    case open(path: String)
    /// Restore this file to before the session changed it.
    case revert(path: String)
    /// Restore the session's code to the checkpoint with this id (DT§5.2, A101); the
    /// conversation stays. `RewindTimeline` offers the other scopes.
    case rewind(checkpoint: String)
  }

  let state: State
  let send: @MainActor (Intent) -> Void

  public init(state: State, send: @escaping @MainActor (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.xl) {
      if !state.files.isEmpty {
        InspectorSection(state.filesTitle) {
          ReviewButton { send(.review) }
        } rows: {
          ForEach(state.files, id: \.path) { file in
            ChangedFileRow(
              file, isSelected: file.path == state.selection,
              actions: Self.fileActions(file.path, send: send)
            )
            .onTapGesture { send(.open(path: file.path)) }
            .accessibilityAction { send(.open(path: file.path)) }
          }
        }
      }
      if !state.checkpoints.isEmpty {
        InspectorSection("Checkpoints") {
          ForEach(state.checkpoints, id: \.id) {
            CheckpointRow($0, actions: Self.checkpointActions($0.id, send: send))
          }
        }
      }
      if !state.worktree.isEmpty {
        InspectorSection("Worktree") { KeyValueGrid(rows: state.worktree) }
      }
      if state == State() {
        Text("No changes yet")
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
      }
    }
  }
}

extension ChangesTab.State {
  /// `This session · 3 files`.
  var filesTitle: String {
    "This session · \(files.count) \(files.count == 1 ? "file" : "files")"
  }
}

extension ChangesTab {
  /// A changed file's row actions: open it in Review, or revert it.
  nonisolated static func fileActions(
    _ path: String, send: @escaping @MainActor (Intent) -> Void
  ) -> [RowAction] {
    [
      RowAction(title: "Review", symbol: "eye") { send(.open(path: path)) },
      RowAction(title: "Revert", symbol: "arrow.uturn.backward") { send(.revert(path: path)) },
    ]
  }

  /// A checkpoint's row action: rewind to it.
  nonisolated static func checkpointActions(
    _ id: String, send: @escaping @MainActor (Intent) -> Void
  ) -> [RowAction] {
    [RowAction(title: "Rewind", symbol: "arrow.uturn.backward") { send(.rewind(checkpoint: id)) }]
  }
}

extension ShellShortcut {
  /// Review's key (DT§5.4). The app's menu command answers it, so the tab's button only names it.
  public static let review = Self(key: .init("r", modifiers: [.command, .shift]), glyphs: "⌘⇧R")
}

/// One block of an inspector tab (the mockup's `.ih` over its rows): a `SectionHeader` with an
/// optional trailing control, then the rows flush under it. Shared by every tab.
struct InspectorSection<Trailing: View, Rows: View>: View {
  let title: String
  let trailing: Trailing
  let rows: Rows

  init(
    _ title: String, @ViewBuilder trailing: () -> Trailing, @ViewBuilder rows: () -> Rows
  ) {
    self.title = title
    self.trailing = trailing()
    self.rows = rows()
  }

  var body: some View {
    VStack(alignment: .leading, spacing: Space.m) {
      SectionHeader(title) { trailing }
      VStack(alignment: .leading, spacing: 0) { rows }
    }
  }
}

extension InspectorSection where Trailing == EmptyView {
  init(_ title: String, @ViewBuilder rows: () -> Rows) {
    self.init(title, trailing: { EmptyView() }, rows: rows)
  }
}

/// The Changes header's `Review` link in `accent`, with its key beside it and in its tooltip.
private struct ReviewButton: View {
  let action: @MainActor () -> Void

  var body: some View {
    Button(action: action) {
      HStack(spacing: Space.xs) {
        Text("Review").textStyle(.control).foregroundStyle(Color(.accent))
        KeyCap(ShellShortcut.review.glyphs)
      }
    }
    .buttonStyle(.plain)
    .help(ShellShortcut.review.help("Review changes"))
    .accessibilityLabel("Review changes")
  }
}

#Preview("changes") { PreviewMatrix { ChangesInspectorSample(state: PreviewState.changes) } }
#Preview("empty") { PreviewMatrix { ChangesInspectorSample(state: .init()) } }
