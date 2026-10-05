// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `BrowserPaneChrome` (DS§6.4 row `BrowserPaneChrome`, mockup 25's web column; T51.10): the
// browser pane beside the transcript. A bar holds back, the address field with its lock and
// reload, over the page in a rounded well. Separate because the page itself is WebKit's
// `WebView`, which CoxUI does not link (DT§4.6). The app puts it in the `content` slot, and
// this draws everything around it. What an address may be is Rust's check, so the field only
// reports what was typed.

import SwiftUI

/// The bar is `Size.toolbarHeight` tall. It holds:
/// - a back chevron in `text.secondary`;
/// - the address in a sunken `fill.primary` well, `font.caption`, with a lock for `https` and
///   a spinner while the page loads;
/// - an icon capsule that reloads.
///
/// Below it, `content` sits in a `radius.xl` well with a hairline rim.
public struct BrowserPaneChrome<Content: View>: View {
  public typealias State = BrowserPaneState
  public typealias Intent = BrowserPaneIntent

  let state: State
  let send: (Intent) -> Void
  let content: Content
  /// What the field shows: the page's address until the person types over it.
  @SwiftUI.State private var draft: String

  public init(
    state: State, send: @escaping (Intent) -> Void, @ViewBuilder content: () -> Content
  ) {
    self.state = state
    self.send = send
    self.content = content()
    _draft = .init(initialValue: state.address)
  }

  public var body: some View {
    VStack(spacing: 0) {
      bar
      content
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipShape(RoundedRectangle(cornerRadius: Radius.xl, style: .continuous))
        .hairline(in: RoundedRectangle(cornerRadius: Radius.xl, style: .continuous))
        .padding([.horizontal, .bottom], Space.l)
    }
    .onChange(of: state.address) { _, address in draft = address }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Browser")
  }

  private var bar: some View {
    HStack(spacing: Space.m) {
      Button {
        send(.back)
      } label: {
        Image(systemName: "chevron.left").symbolStyle(.caption)
      }
      .buttonStyle(.plain)
      .foregroundStyle(Color(.textSecondary))
      .disabled(!state.canGoBack)
      .accessibilityLabel("Back")
      address
      Button {
        send(.reload)
      } label: {
        Image(systemName: "arrow.clockwise").symbolStyle(.caption)
      }
      .buttonStyle(CapsuleStyle(isIcon: true))
      .accessibilityLabel("Reload")
    }
    .padding(.horizontal, Space.l)
    .frame(height: Size.toolbarHeight)
  }

  private var address: some View {
    HStack(spacing: Space.s) {
      if state.isLoading {
        Spinner()
      } else if state.isSecure {
        Image(systemName: "lock.fill")
          .symbolStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .accessibilityLabel("Secure")
      }
      TextField(
        "Address", text: $draft,
        prompt: Text("Address").foregroundStyle(Color(.textPlaceholder))
      )
      .textFieldStyle(.plain)
      .textStyle(.caption)
      .foregroundStyle(Color(.textPrimary))
      .onSubmit { send(.open(draft)) }
    }
    .padding(.horizontal, Space.ml)
    .frame(height: Size.buttonHeight)
    .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
  }
}

/// What `BrowserPaneChrome` shows of the page. It sits outside the generic view so the app and
/// the previews share one type whatever the pane holds.
public struct BrowserPaneState: Equatable, Sendable {
  /// The page's address as the field shows it, `localhost:5173/checkout`.
  public var address: String
  public var isLoading: Bool
  /// The page came over `https`: the field shows a lock.
  public var isSecure: Bool
  public var canGoBack: Bool

  public init(
    address: String = "", isLoading: Bool = false, isSecure: Bool = false,
    canGoBack: Bool = false
  ) {
    (self.address, self.isLoading, self.isSecure, self.canGoBack) =
      (address, isLoading, isSecure, canGoBack)
  }
}

/// What `BrowserPaneChrome`'s bar reports.
public enum BrowserPaneIntent: Equatable, Sendable {
  case back
  case reload
  /// Return in the field: what was typed, for Rust to check and open.
  case open(String)
}

extension ShellShortcut {
  /// The browser pane's toggle (DT§5.5, T51.10). The View menu's command answers it.
  public static let browser = Self(key: .init("b", modifiers: [.command, .shift]), glyphs: "⌘⇧B")
}

#Preview("idle") {
  PreviewMatrix { BrowserPaneSample(state: PreviewState.browserIdle) }
}
#Preview("loading") {
  PreviewMatrix { BrowserPaneSample(state: PreviewState.browserLoading) }
}
#Preview("https") {
  PreviewMatrix { BrowserPaneSample(state: PreviewState.browserSecure) }
}
