// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `BestOfMergeSheet` (DT§3.3.1, T52.26): which finished results of a best-of-n group to merge and
// who merges them — cox on a model, or an external agent. Merge stays blocked until two results
// are checked, as the core refuses fewer (T52.24). Separate from `BestOfCompare` so the compare
// view only reports "Merge" and this sheet owns the choice; CoxModel's `BestOfStore` holds it.

import SwiftUI

/// A checkbox row per mergeable result with its diffstat and cost, the merger picker, the last
/// failure in `status.danger`, then Cancel and Merge.
public struct BestOfMergeSheet: View {
  public struct Row: Equatable, Sendable, Identifiable {
    /// The candidate's place in the group, which `Intent.toggle` names.
    public var id: UInt32
    public var label: String
    public var added: Int
    public var removed: Int
    public var files: Int
    public var cost: String
    public var isChecked: Bool

    public init(
      id: UInt32, label: String, added: Int = 0, removed: Int = 0, files: Int = 0,
      cost: String = "—", isChecked: Bool = false
    ) {
      (self.id, self.label, self.added, self.removed) = (id, label, added, removed)
      (self.files, self.cost, self.isChecked) = (files, cost, isChecked)
    }
  }

  public struct State: Equatable, Sendable {
    public var rows: [Row]
    /// The composer's models and the external agents the best-of control offers; an option that
    /// cannot start here carries why and cannot be chosen.
    public var mergers: [BestOfControl.Option]
    /// The chosen merger's option id; empty before one is chosen.
    public var merger: String
    /// The picker's list is shown under its face.
    public var isMenuOpen: Bool
    /// Why the core refused the last merge.
    public var failure: String?

    public init(
      rows: [Row] = [], mergers: [BestOfControl.Option] = [], merger: String = "",
      isMenuOpen: Bool = false, failure: String? = nil
    ) {
      (self.rows, self.mergers, self.merger) = (rows, mergers, merger)
      (self.isMenuOpen, self.failure) = (isMenuOpen, failure)
    }

    public var checked: Int { rows.filter(\.isChecked).count }

    /// The chosen merger, when it can start.
    var chosen: BestOfControl.Option? {
      mergers.first { $0.id == merger && $0.unavailable == nil }
    }

    /// Two or more results and a merger that can start: the core's rule.
    public var canMerge: Bool { checked >= 2 && chosen != nil }
  }

  public enum Intent: Equatable, Sendable {
    case toggle(UInt32)
    case toggleMenu
    case choose(String)
    case merge
    case cancel
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.l) {
      Text("Merge Results").textStyle(.body).fontWeight(.semibold)
        .foregroundStyle(Color(.textPrimary))
        .accessibilityAddTraits(.isHeader)
      Text(
        "The merger gets the prompt and each checked answer and diff, and works in a new worktree."
      )
      .textStyle(.caption).foregroundStyle(Color(.textSecondary))
      .fixedSize(horizontal: false, vertical: true)
      VStack(alignment: .leading, spacing: Space.xs) {
        ForEach(state.rows) { row in ResultRow(row: row) { send(.toggle(row.id)) } }
      }
      .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
      MergerPicker(state: state, send: send)
      if state.checked < 2 {
        Text("Check at least two results to merge.").textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
      }
      if let failure = state.failure {
        Text(failure).textStyle(.caption).foregroundStyle(Color(.statusDanger))
          .fixedSize(horizontal: false, vertical: true)
      }
      HStack(spacing: Space.s) {
        Spacer(minLength: 0)
        Button("Cancel") { send(.cancel) }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          .keyboardShortcut(.cancelAction)
        Button("Merge") { send(.merge) }
          .buttonStyle(CoxButtonStyle(.primary, size: .small))
          .disabled(!state.canMerge)
          .keyboardShortcut(.defaultAction)
      }
    }
    .padding(Space.xl)
    .frame(width: Size.popoverWidth)
  }
}

/// A checkbox, the result's label, its `+n −m` and its cost.
private struct ResultRow: View {
  let row: BestOfMergeSheet.Row
  let toggle: () -> Void

  var body: some View {
    Button(action: toggle) {
      HStack(spacing: Space.m) {
        Image(systemName: row.isChecked ? "checkmark.square.fill" : "square")
          .symbolStyle(.body)
          .foregroundStyle(Color(row.isChecked ? .accent : .textTertiary))
          .accessibilityHidden(true)
        Text(row.label).textStyle(.body).foregroundStyle(Color(.textPrimary)).lineLimit(1)
        Spacer(minLength: Space.s)
        DiffStat(added: row.added, removed: row.removed)
        Text(row.files == 1 ? "1 file" : "\(row.files) files").textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
        Text(verbatim: row.cost).textStyle(.caption, tabularDigits: true)
          .foregroundStyle(Color(.textSecondary))
      }
      .padding(.horizontal, Space.ml)
      .padding(.vertical, Space.s)
      .contentShape(Rectangle())
    }
    .buttonStyle(.coxPlain(isChosen: row.isChecked))
    .accessibilityAddTraits(row.isChecked ? .isSelected : [])
  }
}

/// "Merge with", the chosen merger's face, and, while open, its models then its agents. The list
/// is drawn here, not a system menu, so a snapshot can show it open.
private struct MergerPicker: View {
  let state: BestOfMergeSheet.State
  let send: (BestOfMergeSheet.Intent) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: Space.s) {
      HStack(spacing: Space.m) {
        Text("Merge with").textStyle(.caption).foregroundStyle(Color(.textSecondary))
        Button {
          send(.toggleMenu)
        } label: {
          HStack(spacing: Space.s) {
            Text(verbatim: state.chosen?.label ?? "Choose a model or agent").lineLimit(1)
            Image(systemName: "chevron.down").symbolStyle(.micro)
          }
          .padding(.horizontal, Space.m)
        }
        .buttonStyle(CoxButtonStyle(.secondary, size: .small))
        .accessibilityValue(state.chosen?.label ?? "")
        Spacer(minLength: 0)
      }
      if state.isMenuOpen {
        VStack(alignment: .leading, spacing: Space.xs) {
          MergerSection(
            "Models", options: state.mergers.filter { !$0.isAgent }, state: state, send: send)
          MergerSection(
            "Agents", options: state.mergers.filter(\.isAgent), state: state, send: send)
        }
        .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
      }
    }
  }
}

private struct MergerSection: View {
  let title: LocalizedStringKey
  let options: [BestOfControl.Option]
  let state: BestOfMergeSheet.State
  let send: (BestOfMergeSheet.Intent) -> Void

  init(
    _ title: LocalizedStringKey, options: [BestOfControl.Option], state: BestOfMergeSheet.State,
    send: @escaping (BestOfMergeSheet.Intent) -> Void
  ) {
    (self.title, self.options, self.state, self.send) = (title, options, state, send)
  }

  var body: some View {
    if !options.isEmpty {
      Text(title).textStyle(.caption).foregroundStyle(Color(.textSecondary))
        .padding(.horizontal, Space.ml).padding(.top, Space.s)
      ForEach(options) { option in
        Button {
          send(.choose(option.id))
        } label: {
          HStack(spacing: Space.m) {
            Text(verbatim: option.label).textStyle(.body).foregroundStyle(Color(.textPrimary))
            if let why = option.unavailable {
              Text(verbatim: why).textStyle(.caption).foregroundStyle(Color(.textSecondary))
                .lineLimit(1)
            }
            Spacer(minLength: Space.s)
            Image(systemName: "checkmark").symbolStyle(.body).foregroundStyle(Color(.accent))
              .opacity(option.id == state.merger ? 1 : 0).accessibilityHidden(true)
          }
          .padding(.horizontal, Space.ml)
          .padding(.vertical, Space.s)
          .contentShape(Rectangle())
        }
        .buttonStyle(.coxPlain(isChosen: option.id == state.merger))
        .disabled(option.unavailable != nil)
        .accessibilityAddTraits(option.id == state.merger ? .isSelected : [])
      }
    }
  }
}

extension BestOfControl.Option {
  /// An external agent, by the id convention the app builds (`agent:<name>`).
  var isAgent: Bool { id.hasPrefix("agent:") }
}

#Preview("merge") {
  PreviewMatrix { BestOfMergeSheet(state: PreviewState.bestOfMergeTwo) { _ in } }
}
