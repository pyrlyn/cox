// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `AcpBanner` (DT§3.3.1, mockup 27's `.notice`, T52.8): the line an external agent's transcript
// opens with — who drives the session, and that its own model, auth and billing apply. Separate
// from `NoticeRow` only to fix the wording in one place; the shape is the notice's.

import SwiftUI

/// A `NoticeRow` with the plug symbol, naming the agent over the Agent Client Protocol.
public struct AcpBanner: View {
  /// What the UI calls the agent: "Claude Agent", never "Claude Code".
  let agent: String

  public init(agent: String) { self.agent = agent }

  public var body: some View {
    NoticeRow(text, symbol: "powerplug")
  }

  /// The agent's name in bold, as mockup 27 sets it; built from runs, never parsed as
  /// Markdown, since the name comes from the user's config or a plugin.
  private var text: AttributedString {
    var name = AttributedString(agent)
    name.inlinePresentationIntent = .stronglyEmphasized
    return AttributedString("This session is driven by ") + name
      + AttributedString(
        " over the Agent Client Protocol. Its own model, auth and billing apply; cox renders "
          + "the stream and answers approvals.")
  }
}

#Preview("banner") { PreviewMatrix { AcpBannerSample() } }
