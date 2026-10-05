// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.8's CoxUI check (DT§3.3.1, mockup 27): the New-session agent picker, picked and after a
// failed read, the ACP banner, the Agents list on the Info tab and the toolbar of an external
// agent's session, each in light/dark × Solid/Frosted.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ExternalAgentSnapshotTests {
  @Test(arguments: Variant.all) func agentPicker(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { AgentPickerSheet(state: PreviewState.agentPicker) { _ in } }, variant,
      named: "picked.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { AgentPickerSheet(state: PreviewState.agentPickerFailed) { _ in } }, variant,
      named: "failed.\(variant.name)")
  }

  @Test(arguments: Variant.all) func acpBanner(_ variant: Variant) throws {
    try assertCoxSnapshot(PreviewPane { AcpBannerSample() }, variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func agentsList(_ variant: Variant) throws {
    try assertCoxSnapshot(
      InfoInspectorSample(state: PreviewState.infoAgents), variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func agentToolbar(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { SessionToolbar(state: PreviewState.toolbarAgent) { _ in }.fixedSize() },
      variant, named: variant.name)
  }
}

@Suite struct ExternalAgentTests {
  @Test func anAgentThatCannotStartSaysWhy() {
    let blocked = PreviewState.agents.filter { $0.unavailable != nil }
    #expect(blocked.map(\.label) == ["Gemini CLI"])
  }

  @Test func theClaudeAdapterIsNeverCalledClaudeCode() {
    #expect(PreviewState.agents.allSatisfy { !$0.label.contains("Claude Code") })
  }
}
