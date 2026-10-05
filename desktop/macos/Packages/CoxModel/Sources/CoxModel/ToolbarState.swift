// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session toolbar's figures (DT§5.1 Toolbar, DS§6.4 `SessionToolbar`): the session's title and
// where it lives, its cost and how full its context is, from the store's meter, the session's row
// in `cox.db` and its Info; an external agent's session names its agent in the model chip and
// has no cost, since its billing is the agent's own (mockup 27). Here, not in CoxUI, because
// these decide what the bar shows (DS§1); the app copies them into `SessionToolbar.State` field
// for field.

import CoxClient
import Foundation

public struct ToolbarState: Equatable, Sendable {
  /// The session's name as the sidebar shows it; `Untitled session` before `cox.db` lists it.
  public var title = SessionEntry.untitled
  /// The project's name and the linked worktree's branch, if any.
  public var project = ""
  public var branch: String?
  /// `$0.42`, the session's cost so far; `—` for an external agent's session, which cox's
  /// ledger never sees.
  public var cost = usd(0)
  /// `Claude Agent · ACP` for an external agent's session; `nil` for cox's own, whose chip shows
  /// the composer's model.
  public var model: String?
  /// `38%` of the window, as the core formatted the share; `–` while the window is unknown.
  public var context = "–"
  /// The share the ring fills, 0…1: the split's parts laid end to end.
  public var contextFraction = 0.0

  public init() {}

  /// `entry` is the session's row once `cox.db` has one; `info` what the session reported;
  /// `agents` what the UI calls each external agent.
  public init(
    usage: UsageView?, entry: (session: SessionEntry, project: Project)?, info: Info?,
    agents: [AgentChoice] = []
  ) {
    if let entry { title = entry.session.name }
    let agent = entry?.session.agent
    if let agent {
      model = "\(agents.label(of: agent)) · ACP"
      cost = "—"
    }
    let cwd = info?.cwd ?? entry?.session.cwd ?? ""
    project = entry?.project.name ?? (cwd.isEmpty ? "" : URL(filePath: cwd).lastPathComponent)
    branch = info?.worktree?.branch
    guard let usage else { return }
    // The core already formatted these; splitting `contextShare` or summing
    // parts here would drift from the meter the moment either figure changes.
    if agent == nil { cost = usage.text.cost }
    if !usage.text.contextPercent.isEmpty { context = usage.text.contextPercent }
    contextFraction = usage.text.contextFill
  }
}

/// `$0.42`, as cox-app's meter and the TUI write a cost.
func usd(_ amount: Double) -> String { String(format: "$%.2f", amount) }
