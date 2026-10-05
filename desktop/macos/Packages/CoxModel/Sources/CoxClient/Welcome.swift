// What an empty session's welcome hero shows (Figma frame 22-empty-session, T37.49): one line
// about the workspace and the suggestions that fill the composer, as cox-app's `welcome` reads
// them from the session's folder. `WelcomeService` is the seam: CoxCore's live client conforms,
// and `FixtureWelcome` gives fixed facts to tests, previews and a client without a core.

/// One suggestion card: its title, the line under it, and the prompt a click drafts.
public struct WelcomeSuggestion: Equatable, Sendable {
  public var title: String
  public var detail: String
  public var prompt: String

  public init(title: String, detail: String, prompt: String) {
    (self.title, self.detail, self.prompt) = (title, detail, prompt)
  }
}

/// The hero's facts about one project.
public struct WelcomeFacts: Equatable, Sendable {
  /// `Rust workspace · 31 crates · AGENTS.md loaded`; empty when nothing is known.
  public var summary: String
  public var suggestions: [WelcomeSuggestion]

  public init(summary: String = "", suggestions: [WelcomeSuggestion] = []) {
    (self.summary, self.suggestions) = (summary, suggestions)
  }
}

/// Where the hero's facts come from.
public protocol WelcomeService: Sendable {
  /// The facts for the session's working folder.
  func welcome(cwd: String) async throws -> WelcomeFacts
}

/// The same facts for every folder.
public struct FixtureWelcome: WelcomeService {
  public var facts: WelcomeFacts

  public init(_ facts: WelcomeFacts = WelcomeFacts()) { self.facts = facts }

  public func welcome(cwd: String) async throws -> WelcomeFacts { facts }
}
