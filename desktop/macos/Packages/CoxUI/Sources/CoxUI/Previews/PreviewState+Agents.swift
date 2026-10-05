// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for mockup 27 (T52.8): cox and three external ACP agents, one of them
// unable to start here, as the New-session sheet, the Info tab's Agents list and the transcript's
// banner show them. Separate so the agents' fixtures do not grow the Info tab's file.

import SwiftUI

extension PreviewState {
  /// Cox, Claude Agent and Codex ready, Gemini CLI missing its program.
  static let agents: [AgentsList.Row] = [
    .init(id: "", label: "cox", origin: "built-in"),
    .init(
      id: "claude", label: "Claude Agent", origin: "user config",
      launch: "claude-agent-acp --hide-claude-auth · ANTHROPIC_API_KEY · network · ~/.claude"),
    .init(
      id: "codex", label: "Codex", origin: "user config",
      launch: "codex-acp · OPENAI_API_KEY · network · ~/.codex"),
    .init(
      id: "gemini", label: "Gemini CLI", origin: "plugin gemini-acp",
      launch: "gemini --experimental-acp · GEMINI_API_KEY · network",
      unavailable: "gemini is not on PATH"),
  ]

  /// The sheet with Claude Agent picked.
  static let agentPicker = AgentPickerSheet.State(rows: agents, selection: "claude")

  /// The sheet after the list could not be read: cox alone, and why.
  static let agentPickerFailed = AgentPickerSheet.State(
    rows: [agents[0]], failure: "config: [external_agents.claude] needs a command")

  /// Mockup 27's toolbar: the agent in the model chip, no cox ledger cost, no context share.
  static let toolbarAgent = SessionToolbar.State(
    title: "Refactor checkout form", project: project, branch: "wt/checkout-form",
    model: "Claude Agent · ACP", cost: "—", context: "–", isRunning: true)

  /// The Info tab of a Claude Agent session.
  static let infoAgents = InfoTab.State(session: infoSession, agents: agents)
}

/// The Agents list at the inspector's width.
struct AgentsListSample: View {
  let rows: [AgentsList.Row]

  var body: some View {
    AgentsList(rows: rows).frame(width: Size.inspectorWidth).padding(Space.xl)
  }
}

/// The banner across a reading column.
struct AcpBannerSample: View {
  var body: some View {
    AcpBanner(agent: "Claude Agent").frame(width: Size.readingWidth).padding(Space.xl)
  }
}
