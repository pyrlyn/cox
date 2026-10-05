// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Badge` (DS§6.2 row `Badge`, the mockup's `.badge .b-*`): a short tag in a soft tint of its
// meaning — where a setting comes from, an agent's model, a warning. Separate so every tag has
// one shape and one colour per kind; `RiskChip` is a badge too.

import SwiftUI

/// Tinted text on a tinted, flat rounded face.
struct Badge: View {
  enum Kind: CaseIterable, Sendable {
    case neutral, user, project, env, `default`, warning, danger
  }

  let text: String
  let foreground: Color
  let background: Color

  /// The mockup's `padding: 1px 6px`: the vertical step is below `Space.xxs`.
  private static let verticalPadding: CGFloat = 1

  init(_ text: String, kind: Kind = .neutral) {
    self.init(text, foreground: kind.foreground, background: kind.background)
  }

  /// A badge in its caller's roles, as `RiskChip` draws a risk level.
  init(_ text: String, foreground: Color, background: Color) {
    self.text = text
    self.foreground = foreground
    self.background = background
  }

  var body: some View {
    Text(text)
      .textStyle(.micro)
      .lineLimit(1)
      .foregroundStyle(foreground)
      .padding(.horizontal, Space.s)
      .padding(.vertical, Self.verticalPadding)
      .background(background, in: RoundedRectangle(cornerRadius: Radius.badge, style: .continuous))
  }
}

extension Badge.Kind {
  var foreground: Color {
    switch self {
    case .neutral, .default: Color(.textSecondary)
    case .user: Color(.accent)
    case .project: Color(.roleProject)
    case .env: Color(.statusSuccess)
    case .warning: Color(.statusWarning)
    case .danger: Color(.statusDanger)
    }
  }

  var background: Color {
    switch self {
    case .neutral, .default: Color(.fillSecondary)
    case .user: Color(.accentSoft)
    case .project: Color(.roleProjectSoft)
    case .env: Color(.statusSuccessSoft)
    case .warning: Color(.statusWarningSoft)
    case .danger: Color(.statusDangerSoft)
    }
  }
}

#Preview("neutral") { PreviewMatrix { Badge(PreviewState.badge(.neutral), kind: .neutral) } }
#Preview("user") { PreviewMatrix { Badge(PreviewState.badge(.user), kind: .user) } }
#Preview("project") { PreviewMatrix { Badge(PreviewState.badge(.project), kind: .project) } }
#Preview("env") { PreviewMatrix { Badge(PreviewState.badge(.env), kind: .env) } }
#Preview("default") { PreviewMatrix { Badge(PreviewState.badge(.default), kind: .default) } }
#Preview("warning") { PreviewMatrix { Badge(PreviewState.badge(.warning), kind: .warning) } }
#Preview("danger") { PreviewMatrix { Badge(PreviewState.badge(.danger), kind: .danger) } }
