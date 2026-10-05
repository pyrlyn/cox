// An empty session's welcome (Figma frame 22-empty-session): CoxUI's `WelcomeHero` over the
// transcript until the first block, with the facts CoxClient's `WelcomeService` gives for the session's
// folder, a suggestion filling the composer's draft. Here because the hero meets the composer's
// store only in this package; the app decides when the transcript is empty.

import CoxClient
import CoxModel
import CoxUI
import SwiftUI

/// The hero for one session's folder; the facts load once per folder.
public struct SessionWelcome: View {
  let project: String
  let cwd: String
  let composer: ComposerStore
  let service: any WelcomeService
  @State private var facts = WelcomeFacts()

  public init(
    project: String, cwd: String, composer: ComposerStore, service: any WelcomeService
  ) {
    (self.project, self.cwd, self.composer, self.service) = (project, cwd, composer, service)
  }

  public var body: some View {
    WelcomeHero(state: Self.state(project: project, facts: facts)) { intent in
      switch intent {
      case .suggest(let prompt):
        if let pick = facts.suggestions.first(where: { $0.prompt == prompt }) {
          composer.suggest(pick)
        }
      }
    }
    .task(id: cwd) {
      // A folder the service cannot read keeps the question alone, never an error.
      facts = (try? await service.welcome(cwd: cwd)) ?? WelcomeFacts()
    }
  }

  /// The facts as the hero's state, field for field.
  static func state(project: String, facts: WelcomeFacts) -> WelcomeHero.State {
    WelcomeHero.State(
      project: project, summary: facts.summary,
      suggestions: facts.suggestions.map {
        WelcomeHero.Suggestion(title: $0.title, detail: $0.detail, prompt: $0.prompt)
      })
  }
}
