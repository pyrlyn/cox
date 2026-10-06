// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What `FixtureSession` reads off a patch batch and an intent to know which approvals and
// questions are pending (DT§8). Its own file so the client seam stays one screen of protocol.

extension [TimelinePatch] {
  /// The approvals and questions this batch leaves pending.
  var waiting: Set<String> {
    var calls: Set<String> = []
    for case .upsert(let block, _) in self {
      switch block.kind {
      case .approval(let call, _, _, _, _, _, _, let decision, _):
        if decision == nil { calls.insert(call) } else { calls.remove(call) }
      case .question(let call, _, _, let answer):
        if answer == nil { calls.insert(call) } else { calls.remove(call) }
      default: break
      }
    }
    return calls
  }
}

extension Intent {
  /// The approval or question this intent answers.
  var answers: String? {
    switch self {
    case .approve(let call, _): call
    case .answer(let question, _): question
    default: nil
    }
  }
}
