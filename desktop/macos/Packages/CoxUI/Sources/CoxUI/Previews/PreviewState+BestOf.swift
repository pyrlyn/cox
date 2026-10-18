// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for best of n (T52.12, mockup 27): the composer's control with Codex
// added, and the compare view with two and three candidates, one of which failed to start, plus
// both questions "Keep this one" asks. Separate so these do not grow the agents' file.

import SwiftUI

extension PreviewState {
  /// Mockup 27: Codex added, cox on Sonnet 5 offered, Gemini CLI missing its program.
  static let bestOfControl = BestOfControl.State(options: [
    .init(id: "agent:codex", label: "Codex", isPicked: true),
    .init(id: "cox:claude-sonnet-5", label: "cox · Sonnet 5"),
    .init(id: "agent:gemini", label: "Gemini CLI", unavailable: "gemini is not on PATH"),
  ])

  /// The session's provider has no key: the capsule is disabled and the model on a provider with
  /// no key cannot be added (T60.5). Codex does not need one and stays added, so the capsule is
  /// disabled with two candidates, not for want of a second.
  static let bestOfNotReady = BestOfControl.State(
    options: [
      .init(id: "agent:codex", label: "Codex", isPicked: true),
      .init(
        id: "cox:claude-sonnet-5", label: "cox · Sonnet 5",
        unavailable: "No key for anthropic, or it is not running."),
    ], unavailable: "No API key for anthropic.")

  private static let prompt = "Split CheckoutForm into address, payment and summary components."

  private static let claudeColumn = BestOfCompare.Column(
    id: 0, label: "Claude Agent", status: .done, branch: "best-01k6x-1",
    files: [
      .init(path: "src/checkout/AddressFields.tsx", added: 64, removed: 0),
      .init(path: "src/checkout/PaymentFields.tsx", added: 58, removed: 0),
      .init(path: "src/checkout/CheckoutForm.tsx", added: 12, removed: 131),
    ],
    duration: "4m 12s", canReview: true, canKeep: true)

  private static let coxColumn = BestOfCompare.Column(
    id: 1, label: "cox · Sonnet 5", status: .running, branch: "best-01k6x-2",
    files: [
      .init(path: "src/checkout/AddressFields.tsx", added: 71, removed: 0),
      .init(path: "src/checkout/CheckoutForm.tsx", added: 9, removed: 88),
    ],
    cost: "$0.38", duration: "3m 05s", canReview: true)

  private static let codexColumn = BestOfCompare.Column(
    id: 2, label: "Codex", status: .failed("CODEX_API_KEY is not set"))

  /// Two candidates: one done, one that did not start.
  static let bestOfTwo = BestOfCompare.State(
    prompt: prompt, total: "$0.00", columns: [claudeColumn, codexColumn])

  /// Three candidates: one done, one running, one that did not start.
  static let bestOfThree = BestOfCompare.State(
    prompt: prompt, total: "$0.38", columns: [claudeColumn, coxColumn, codexColumn])

  /// A candidate whose turn failed after it started, as Best of reads it back: the provider's
  /// own error, no files, nothing to review or keep (T60.10).
  private static let failedTurnColumn = BestOfCompare.Column(
    id: 2, label: "cox · Opus 5", status: .failed("provider error: provider auth failed"),
    branch: "best-01k6x-3", cost: "$0.00", duration: "0s")

  /// Every candidate's turn failed on the provider: both columns show the error, and neither
  /// offers an action.
  static let bestOfFailedTurns = BestOfCompare.State(
    prompt: prompt, total: "$0.00",
    columns: [
      BestOfCompare.Column(
        id: 1, label: "cox · Sonnet 5", status: .failed("provider error: provider auth failed"),
        branch: "best-01k6x-2", cost: "$0.00", duration: "0s"),
      failedTurnColumn,
    ])

  /// "Keep this one" on Claude Agent: the worktree it prunes.
  static let bestOfConfirm = BestOfCompare.State(
    prompt: prompt, total: "$0.38", columns: [claudeColumn, coxColumn, codexColumn],
    confirm: .init(keep: 0, label: "Claude Agent", prunes: ["cox · Sonnet 5 · best-01k6x-2"]))

  /// The second question: the pruned worktree holds changes.
  static let bestOfDirty = BestOfCompare.State(
    prompt: prompt, total: "$0.38", columns: [claudeColumn, coxColumn, codexColumn],
    confirm: .init(
      keep: 0, label: "Claude Agent", prunes: ["cox · Sonnet 5 · best-01k6x-2"],
      dirty: ["/work/_worktrees/shop-best-01k6x-2"]),
    notes: ["cox · Sonnet 5 is still running; stop it first"])

  private static let doneCox = BestOfCompare.Column(
    id: 1, label: "cox · Sonnet 5", status: .done, branch: "best-01k6x-2",
    files: [
      .init(path: "src/checkout/AddressFields.tsx", added: 71, removed: 0),
      .init(path: "src/checkout/CheckoutForm.tsx", added: 9, removed: 88),
    ],
    cost: "$0.52", duration: "5m 20s", canReview: true, canKeep: true)

  private static let doneCodex = BestOfCompare.Column(
    id: 2, label: "Codex", status: .done, branch: "best-01k6x-3",
    files: [.init(path: "src/checkout/SummaryPanel.tsx", added: 47, removed: 0)],
    duration: "3m 48s", canReview: true, canKeep: true)

  /// One result and one that did not start: nothing to merge, so no Merge button (T52.26).
  static let bestOfOneResult = BestOfCompare.State(
    prompt: prompt, total: "$0.00", columns: [claudeColumn, codexColumn], mergeable: 1)

  /// Three results: the core lists all of them, and Merge shows.
  static let bestOfThreeResults = BestOfCompare.State(
    prompt: prompt, total: "$0.52", columns: [claudeColumn, doneCox, doneCodex], mergeable: 3)

  private static func merged(
    _ status: BestOfCompare.Status, cost: String, duration: String
  ) -> BestOfCompare.Column {
    BestOfCompare.Column(
      id: 3, label: "cox · Opus 5", status: status, branch: "best-01k6x-4",
      files: [
        .init(path: "src/checkout/AddressFields.tsx", added: 71, removed: 0),
        .init(path: "src/checkout/CheckoutForm.tsx", added: 12, removed: 131),
      ],
      cost: cost, duration: duration, canReview: true, canKeep: status == .done,
      merges: ["Claude Agent", "cox · Sonnet 5"])
  }

  /// The merge as one more column while its merger works.
  static let bestOfMergeRunning = BestOfCompare.State(
    prompt: prompt, total: "$0.61",
    columns: [claudeColumn, doneCox, doneCodex, merged(.running, cost: "$0.09", duration: "40s")],
    mergeable: 3)

  /// The merge finished: it can be opened in Review or kept like any other candidate.
  static let bestOfMergeDone = BestOfCompare.State(
    prompt: prompt, total: "$0.94",
    columns: [claudeColumn, doneCox, doneCodex, merged(.done, cost: "$0.42", duration: "2m 10s")],
    mergeable: 4)

  private static let mergeRows = [
    BestOfMergeSheet.Row(
      id: 0, label: "Claude Agent", added: 134, removed: 131, files: 3, cost: "—"),
    BestOfMergeSheet.Row(
      id: 1, label: "cox · Sonnet 5", added: 80, removed: 88, files: 2, cost: "$0.52"),
    BestOfMergeSheet.Row(id: 2, label: "Codex", added: 47, removed: 0, files: 1, cost: "—"),
  ]

  private static func checked(_ ids: Set<UInt32>) -> [BestOfMergeSheet.Row] {
    mergeRows.map { row in
      var row = row
      row.isChecked = ids.contains(row.id)
      return row
    }
  }

  /// The composer's models and the agents the best-of control offers; Gemini is missing its
  /// program and cannot merge.
  private static let mergers = [
    BestOfControl.Option(id: "cox:claude-sonnet-5", label: "cox · Sonnet 5"),
    BestOfControl.Option(id: "cox:claude-opus-5", label: "cox · Opus 5"),
    BestOfControl.Option(id: "agent:codex", label: "Codex"),
    BestOfControl.Option(id: "agent:gemini", label: "Gemini CLI", unavailable: "not on PATH"),
  ]

  /// Two results checked and a model chosen: Merge is ready.
  static let bestOfMergeTwo = BestOfMergeSheet.State(
    rows: checked([0, 1]), mergers: mergers, merger: "cox:claude-opus-5")

  /// One result checked: Merge is blocked and the sheet says why.
  static let bestOfMergeOne = BestOfMergeSheet.State(
    rows: checked([2]), mergers: mergers, merger: "cox:claude-opus-5")

  /// The picker open with a model chosen.
  static let bestOfMergeModelMenu = BestOfMergeSheet.State(
    rows: checked([0, 1]), mergers: mergers, merger: "cox:claude-opus-5", isMenuOpen: true)

  /// The picker open with an external agent chosen.
  static let bestOfMergeAgentMenu = BestOfMergeSheet.State(
    rows: checked([0, 1]), mergers: mergers, merger: "agent:codex", isMenuOpen: true)
}
