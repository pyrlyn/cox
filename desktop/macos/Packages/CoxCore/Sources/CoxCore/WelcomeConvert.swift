// An empty session's welcome hero facts over cox-ffi (Figma frame 22-empty-session, T37.49):
// `App.welcome` as CoxClient's `WelcomeFacts`. cox-app reads the folder's manifest and the
// instruction files a session there loads; this only converts.

import CoxClient
import CoxFFIBindings

extension LiveCoreClient: WelcomeService {
  public func welcome(cwd: String) async throws -> WelcomeFacts {
    let welcome = try await app.welcome(cwd: cwd)
    return WelcomeFacts(
      summary: welcome.summary,
      suggestions: welcome.suggestions.map {
        WelcomeSuggestion(title: $0.title, detail: $0.detail, prompt: $0.prompt)
      })
  }
}
