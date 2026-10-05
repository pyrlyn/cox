// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `BestOfCompare` (DT§3.3.1, T52.12): one best-of-n group side by side — a column per candidate
// with its state, the files its worktree changed, its cost and how long it ran, "Open in Review"
// and "Keep this one". Keeping one first lists the worktrees it prunes; a worktree with changes
// that are not committed is asked about a second time. Separate so the app only presents it:
// CoxModel's `BestOfStore` owns the columns and the pick, the view reports intents.

import SwiftUI

public struct BestOfCompare: View {
  public enum Status: Equatable, Sendable {
    case running, waitingOnYou, done
    case failed(String)
    case kept, pruned
  }

  public struct File: Equatable, Sendable, Identifiable {
    public var path: String
    public var added: Int
    public var removed: Int

    public var id: String { path }

    public init(path: String, added: Int, removed: Int) {
      (self.path, self.added, self.removed) = (path, added, removed)
    }
  }

  public struct Column: Equatable, Sendable, Identifiable {
    /// The candidate's place in the group, which `Intent.keep` names.
    public var id: UInt32
    public var label: String
    public var status: Status
    public var branch: String?
    public var files: [File]
    public var added: Int
    public var removed: Int
    /// "$0.42", or "—" for an agent that bills itself.
    public var cost: String
    public var duration: String
    public var canReview: Bool
    public var canKeep: Bool

    public init(
      id: UInt32, label: String, status: Status, branch: String? = nil, files: [File] = [],
      cost: String = "—", duration: String = "", canReview: Bool = false, canKeep: Bool = false
    ) {
      (self.id, self.label, self.status, self.branch, self.files) =
        (id, label, status, branch, files)
      (self.added, self.removed) = (
        files.reduce(0) { $0 + $1.added }, files.reduce(0) { $0 + $1.removed }
      )
      (self.cost, self.duration, self.canReview, self.canKeep) =
        (cost, duration, canReview, canKeep)
    }
  }

  /// "Keep this one" asked: what it prunes, then, if a first yes left some, the worktrees that
  /// hold changes.
  public struct Confirm: Equatable, Sendable {
    public var keep: UInt32
    public var label: String
    public var prunes: [String]
    public var dirty: [String]

    public init(keep: UInt32, label: String, prunes: [String], dirty: [String] = []) {
      (self.keep, self.label, self.prunes, self.dirty) = (keep, label, prunes, dirty)
    }
  }

  public struct State: Equatable, Sendable {
    public var prompt: String
    /// The group's cost, "$1.24".
    public var total: String
    public var columns: [Column]
    public var confirm: Confirm?
    /// Why the last read or pick failed, and why a pick left some worktrees.
    public var notes: [String]

    public init(
      prompt: String = "", total: String = "", columns: [Column] = [], confirm: Confirm? = nil,
      notes: [String] = []
    ) {
      (self.prompt, self.total, self.columns, self.confirm, self.notes) =
        (prompt, total, columns, confirm, notes)
    }
  }

  public enum Intent: Equatable, Sendable {
    case review(UInt32)
    case keep(UInt32)
    /// The first yes: keep and prune.
    case confirm
    /// The second yes: discard the changes too.
    case discard
    case cancel
    case close
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.l) {
      HStack(alignment: .firstTextBaseline, spacing: Space.m) {
        Text("Best of \(state.columns.count)").textStyle(.body).fontWeight(.semibold)
          .foregroundStyle(Color(.textPrimary))
          .accessibilityAddTraits(.isHeader)
        Spacer(minLength: 0)
        Text(verbatim: state.total).textStyle(.caption, tabularDigits: true)
          .foregroundStyle(Color(.textSecondary))
        Button("Done") { send(.close) }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          .keyboardShortcut(.cancelAction)
      }
      Text(state.prompt).textStyle(.caption).foregroundStyle(Color(.textSecondary))
        .lineLimit(2)
      HStack(alignment: .top, spacing: Space.l) {
        ForEach(state.columns) { column in
          CandidateColumn(column: column, send: send)
        }
      }
      .fixedSize(horizontal: false, vertical: true)
      if let confirm = state.confirm {
        KeepConfirmation(confirm: confirm, send: send)
      }
      ForEach(state.notes, id: \.self) { note in
        NoticeRow(note, kind: .warning)
      }
    }
    .padding(Space.xl)
    .frame(minWidth: Size.readingWidth, alignment: .topLeading)
  }
}

/// One candidate: its label and state, `+n −m` over its files, cost and time, then its actions.
private struct CandidateColumn: View {
  /// Past this many files the column says how many more.
  static let shownFiles = 5

  let column: BestOfCompare.Column
  let send: (BestOfCompare.Intent) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: Space.m) {
      Text(column.label).textStyle(.body).fontWeight(.semibold)
        .foregroundStyle(Color(.textPrimary))
      CandidateStatus(status: column.status)
      if let branch = column.branch {
        Text(verbatim: branch).textStyle(.monoInline).foregroundStyle(Color(.textSecondary))
          .lineLimit(1).truncationMode(.middle)
      }
      HStack(spacing: Space.m) {
        DiffStat(added: column.added, removed: column.removed)
        Text(column.files.count == 1 ? "1 file" : "\(column.files.count) files")
          .textStyle(.caption).foregroundStyle(Color(.textSecondary))
      }
      ForEach(column.files.prefix(Self.shownFiles)) { file in
        HStack(spacing: Space.s) {
          Text(verbatim: file.path).textStyle(.monoInline).foregroundStyle(Color(.textPrimary))
            .lineLimit(1).truncationMode(.middle)
          Spacer(minLength: Space.s)
          DiffStat(added: file.added, removed: file.removed)
        }
      }
      if column.files.count > Self.shownFiles {
        Text("\(column.files.count - Self.shownFiles) more").textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
      }
      Text(
        verbatim: [column.cost, column.duration].filter { !$0.isEmpty }.joined(separator: " · ")
      )
      .textStyle(.caption, tabularDigits: true)
      .foregroundStyle(Color(.textSecondary))
      Spacer(minLength: 0)
      Button("Open in Review") { send(.review(column.id)) }
        .buttonStyle(CoxButtonStyle(.secondary, size: .small))
        .disabled(!column.canReview)
      Button("Keep this one") { send(.keep(column.id)) }
        .buttonStyle(CoxButtonStyle(.primary, size: .small))
        .disabled(!column.canKeep)
    }
    .padding(Space.l)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
  }
}

/// A symbol in the state's colour and its name; a failure's reason under it.
private struct CandidateStatus: View {
  let status: BestOfCompare.Status

  var body: some View {
    VStack(alignment: .leading, spacing: Space.xs) {
      Label {
        Text(name).textStyle(.caption).foregroundStyle(Color(.textPrimary))
      } icon: {
        Image(systemName: symbol).symbolStyle(.body).foregroundStyle(tint)
      }
      if case .failed(let why) = status {
        Text(why).textStyle(.caption).foregroundStyle(Color(.statusDanger))
          .fixedSize(horizontal: false, vertical: true)
      }
    }
  }

  private var name: String {
    switch status {
    case .running: "Running"
    case .waitingOnYou: "Waiting on you"
    case .done: "Done"
    case .failed: "Failed"
    case .kept: "Kept"
    case .pruned: "Pruned"
    }
  }

  private var symbol: String {
    switch status {
    case .running: "circle.dotted"
    case .waitingOnYou: "hand.raised"
    case .done: "checkmark.circle"
    case .failed: "xmark.octagon"
    case .kept: "star.fill"
    case .pruned: "trash"
    }
  }

  private var tint: Color {
    switch status {
    case .running, .kept: Color(.accent)
    case .waitingOnYou: Color(.statusWarning)
    case .done: Color(.statusSuccess)
    case .failed: Color(.statusDanger)
    case .pruned: Color(.textTertiary)
    }
  }
}

/// The first question lists what keeping prunes; the second, the worktrees whose changes a
/// prune would discard.
private struct KeepConfirmation: View {
  let confirm: BestOfCompare.Confirm
  let send: (BestOfCompare.Intent) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: Space.m) {
      Text(question).textStyle(.body).foregroundStyle(Color(.textPrimary))
        .fixedSize(horizontal: false, vertical: true)
      ForEach(listed, id: \.self) { tree in
        Text(verbatim: tree).textStyle(.monoInline).foregroundStyle(Color(.textSecondary))
          .lineLimit(1).truncationMode(.middle)
      }
      HStack(spacing: Space.s) {
        Spacer(minLength: 0)
        if confirm.dirty.isEmpty {
          Button("Cancel") { send(.cancel) }
            .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          Button("Keep and Prune") { send(.confirm) }
            .buttonStyle(CoxButtonStyle(.primary, size: .small))
            .keyboardShortcut(.defaultAction)
        } else {
          Button("Leave Them") { send(.cancel) }
            .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          Button("Discard Changes") { send(.discard) }
            .buttonStyle(CoxButtonStyle(.danger, size: .small))
        }
      }
    }
    .padding(Space.l)
    .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
  }

  private var question: String {
    if !confirm.dirty.isEmpty {
      return "These worktrees hold changes that are not committed. Discard them?"
    }
    if confirm.prunes.isEmpty { return "Keep \(confirm.label)? Nothing else is pruned." }
    return "Keep \(confirm.label)? This removes the other worktrees:"
  }

  private var listed: [String] { confirm.dirty.isEmpty ? confirm.prunes : confirm.dirty }
}

#Preview("three") {
  PreviewMatrix { BestOfCompare(state: PreviewState.bestOfThree) { _ in } }
}
