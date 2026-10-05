// `WelcomeHero` (Figma frame 22-empty-session, the mockup's screen 22): what an empty session's
// transcript column shows before the first prompt — the app icon, "What should we do in
// <project>?", one line about the workspace, and three suggestions that fill the composer.
// Separate so the transcript only decides when it is empty; the facts and the suggestions are
// the caller's (CoxClient's `WelcomeService`, filled by cox-app).

import SwiftUI

/// Centred in the reading column, `Size.welcomeTop` below its top; the suggestions are cards in
/// a three-column grid, each a button that reports its prompt.
public struct WelcomeHero: View {
  public struct State: Equatable, Sendable {
    /// The project's name, as the toolbar's breadcrumb shows it.
    public var project: String
    /// `Rust workspace · 31 crates · AGENTS.md loaded`; empty hides the line.
    public var summary: String
    public var suggestions: [Suggestion]

    public init(project: String, summary: String = "", suggestions: [Suggestion] = []) {
      (self.project, self.summary, self.suggestions) = (project, summary, suggestions)
    }
  }

  /// One card: its bold title, the line under it, and the prompt a click puts in the composer.
  public struct Suggestion: Equatable, Sendable, Identifiable {
    public var id: String { title }
    public var title: String
    public var detail: String
    public var prompt: String

    public init(title: String, detail: String, prompt: String) {
      (self.title, self.detail, self.prompt) = (title, detail, prompt)
    }
  }

  public enum Intent: Equatable, Sendable {
    /// A suggestion was clicked: its prompt goes to the composer's draft.
    case suggest(prompt: String)
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    self.state = state
    self.send = send
  }

  public var body: some View {
    VStack(spacing: 0) {
      AppIcon().padding(.bottom, Space.welcome)
      Text("What should we do in \(state.project)?")
        .textStyle(.titleWelcome)
        .foregroundStyle(Color(.textPrimary))
        .multilineTextAlignment(.center)
        .accessibilityAddTraits(.isHeader)
        .padding(.bottom, Space.xs)
      if !state.summary.isEmpty {
        Text(state.summary)
          .textStyle(.body)
          .foregroundStyle(Color(.textSecondary))
          .multilineTextAlignment(.center)
      }
      if !state.suggestions.isEmpty {
        Grid(horizontalSpacing: Space.ml, verticalSpacing: Space.ml) {
          GridRow {
            ForEach(state.suggestions) { suggestion in
              SuggestionCard(suggestion: suggestion) { send(.suggest(prompt: suggestion.prompt)) }
            }
          }
        }
        // Each card as tall as the row's tallest, the row as tall as its text.
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, Space.xxxl)
      }
    }
    .frame(maxWidth: Size.readingWidth)
    .padding(.top, Size.welcomeTop)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
  }
}

/// The mockup's `.card`: a hairline-bordered rounded card, title over a secondary line.
struct SuggestionCard: View {
  let suggestion: WelcomeHero.Suggestion
  let action: () -> Void

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xl, style: .continuous)
    Button(action: action) {
      VStack(alignment: .leading, spacing: Space.xs) {
        Text(suggestion.title)
          .textStyle(.titleCard)
          .foregroundStyle(Color(.textPrimary))
        Text(suggestion.detail)
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .fixedSize(horizontal: false, vertical: true)
      }
      .multilineTextAlignment(.leading)
      .padding(.horizontal, Space.welcome)
      .padding(.vertical, Space.l)
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
      .contentShape(shape)
    }
    .buttonStyle(.plain)
    .hairline(in: shape)
    .accessibilityHint(suggestion.detail)
  }
}

#Preview("welcome") { WelcomeHeroSample(state: PreviewState.welcome) }
#Preview("no suggestions") { WelcomeHeroSample(state: .init(project: "cox")) }
