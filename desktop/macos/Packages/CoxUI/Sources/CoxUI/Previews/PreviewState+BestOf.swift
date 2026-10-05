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
}
