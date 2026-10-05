// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TerminalPaneChrome` (DS§6.4 row `TerminalPaneChrome`, mockup 24's `.panehead` over `.term`;
// T51.6): the terminal pane under the transcript column — a header of the session's shell tabs,
// `+` for another and the ⌃` hint, over the terminal well. Separate because the terminal itself
// is SwiftTerm's view, which CoxUI does not link (DT§4.6): the app puts it in the `content`
// slot, and this draws everything around it.

import SwiftUI

/// The header on `fill.primary` under a top hairline, `font.caption` in `text.secondary`: a
/// tab per shell, the shown one lifted on `surface.window` in `text.primary` (e1, `radius.s`),
/// then `+` and, trailing, the toggle's keys. Below, `content` on `surface.terminal`.
public struct TerminalPaneChrome<Content: View>: View {
  public typealias Tab = TerminalPaneTab
  public typealias State = TerminalPaneState
  public typealias Intent = TerminalPaneIntent

  let state: State
  let send: (Intent) -> Void
  let content: Content

  public init(
    state: State, send: @escaping (Intent) -> Void, @ViewBuilder content: () -> Content
  ) {
    self.state = state
    self.send = send
    self.content = content()
  }

  public var body: some View {
    VStack(spacing: 0) {
      header
      content
        .padding(.horizontal, Space.l)
        .padding(.vertical, Space.ml)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color(.surfaceTerminal))
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Terminal")
  }

  private var header: some View {
    HStack(spacing: Space.ml) {
      ForEach(state.tabs) { tab in
        TerminalTabButton(title: tab.title, isShown: tab.id == state.selection) {
          send(.select(tab.id))
        }
        .contextMenu { Button("Close Terminal") { send(.close(tab.id)) } }
      }
      TerminalTabButton(symbol: "plus", label: "New terminal", isShown: false) { send(.add) }
      Spacer(minLength: 0)
      Text("\(ShellShortcut.terminal.glyphs) toggle").accessibilityHidden(true)
    }
    .textStyle(.caption)
    .foregroundStyle(Color(.textSecondary))
    .lineLimit(1)
    .padding(.horizontal, Space.l)
    .padding(.vertical, Space.s)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(Color(.fillPrimary))
    .hairline(.top)
  }
}

/// One shell's tab in `TerminalPaneChrome`'s header.
public struct TerminalPaneTab: Identifiable, Equatable, Sendable {
  public let id: Int
  /// `zsh — wt/retry-jitter`, as CoxModel's `TerminalTab.title` makes it.
  public let title: String

  public init(id: Int, title: String) {
    (self.id, self.title) = (id, title)
  }
}

/// `TerminalPaneChrome`'s tabs and the one it shows; outside the generic view so the app and
/// the previews share one type whatever the pane holds.
public struct TerminalPaneState: Equatable, Sendable {
  public var tabs: [TerminalPaneTab]
  public var selection: Int?

  public init(tabs: [TerminalPaneTab] = [], selection: Int? = nil) {
    (self.tabs, self.selection) = (tabs, selection)
  }
}

/// What `TerminalPaneChrome`'s header reports.
public enum TerminalPaneIntent: Equatable, Sendable {
  case select(Int)
  /// `+`: another shell of the same session.
  case add
  case close(Int)
}

/// One tab of the header: a shell's title after the terminal symbol, or `+` alone.
private struct TerminalTabButton: View {
  let title: String?
  let symbol: String
  let label: String
  let isShown: Bool
  let action: () -> Void

  init(title: String, isShown: Bool, action: @escaping () -> Void) {
    (self.title, symbol, label, self.isShown, self.action) =
      (title, "terminal", title, isShown, action)
  }

  init(symbol: String, label: String, isShown: Bool, action: @escaping () -> Void) {
    (title, self.symbol, self.label, self.isShown, self.action) =
      (nil, symbol, label, isShown, action)
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.s, style: .continuous)
    Button(action: action) {
      HStack(spacing: Space.xs) {
        Image(systemName: symbol).symbolStyle(.caption)
        if let title { Text(title).truncationMode(.middle) }
      }
      .padding(.horizontal, Space.m)
      .padding(.vertical, Space.xxs)
      .foregroundStyle(isShown ? Color(.textPrimary) : Color(.textSecondary))
      .background { if isShown { shape.fill(Color(.surfaceWindow)) } }
      .elevation(isShown ? .e1 : .e0, cornerRadius: Radius.s)
      .contentShape(shape)
    }
    .buttonStyle(.plain)
    .accessibilityLabel(label)
    .accessibilityAddTraits(isShown ? .isSelected : [])
  }
}

extension ShellShortcut {
  /// The terminal pane's toggle (DT§5.5, T51.6); the View menu's command answers it.
  public static let terminal = Self(key: .init("`", modifiers: .control), glyphs: "⌃`")
}

#Preview("one tab") {
  PreviewMatrix { TerminalPaneSample(state: PreviewState.terminalOneTab) }
}
#Preview("two tabs") {
  PreviewMatrix { TerminalPaneSample(state: PreviewState.terminalTwoTabs) }
}
