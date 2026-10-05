// The welcome hero's state (Figma frame 22-empty-session): the service's facts reach CoxUI's
// `WelcomeHero.State` field for field, under the toolbar's project name.

import CoxClient
import CoxModel
import CoxUI
import Testing

@testable import CoxTranscript

@MainActor
@Test func theWelcomeFactsBecomeTheHerosStateFieldForField() async throws {
  let facts = try await FixtureWelcome(
    WelcomeFacts(
      summary: "Rust workspace · 31 crates · AGENTS.md loaded",
      suggestions: [
        WelcomeSuggestion(
          title: "Explain the architecture", detail: "How do the parts of cox fit together?",
          prompt: "Explain the architecture: how do the parts of cox fit together?"),
        WelcomeSuggestion(
          title: "Find and fix a failing test",
          detail: "Run cargo nextest and fix the first failure",
          prompt: "Run cargo nextest and fix the first failure."),
      ])
  ).welcome(cwd: "/w/cox")
  let state = SessionWelcome.state(project: "cox", facts: facts)
  #expect(state.project == "cox")
  #expect(state.summary == facts.summary)
  #expect(state.suggestions.map(\.prompt) == facts.suggestions.map(\.prompt))
  #expect(state.suggestions.map(\.title) == facts.suggestions.map(\.title))
}
