// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `MenuBarPanel` (DS§6.4 row `MenuBarPanel`, mockup 26's popover; T51.13): what the menu-bar
// extra shows. "Needs you" rows come first: an approval shows its command with Allow and Deny,
// and a question opens the app. "Running" rows follow, then today's spend, New session and Open
// cox. Allow for the session and editing a command stay in the app, as for notifications
// (DT§5.6). Separate so the extra draws from one value the core fills, and reports every click
// as an intent.

import SwiftUI

/// Rows in `font.body` on the extra's own window. Each section is headed by a `SectionHeader`,
/// and a `Hairline` separates the sections. A waiting row leads with a `StatusDot`, a running
/// one with a `Spinner`. Detail lines are `font.footnote` in `text.secondary`, and a command is
/// `font.monoInline`.
public struct MenuBarPanel: View {
  public struct State: Equatable, Sendable {
    public var needs: [Need]
    public var running: [Running]
    /// Mockup 26's footer figures after "Today": `$4.02 · 7 sessions`.
    public var today: String

    public init(needs: [Need] = [], running: [Running] = [], today: String = "") {
      (self.needs, self.running, self.today) = (needs, running, today)
    }
  }

  /// What a `Need` asks for; one level up so the types nest only one deep.
  public enum NeedKind: Equatable, Sendable {
    /// A tool call to allow or deny, with the command it runs.
    case approval(command: String)
    /// A question, with what it asks; answering it happens in the app.
    case question(String)
  }

  /// One inbox item waiting on the person.
  public struct Need: Identifiable, Equatable, Sendable {
    /// The inbox item's id, which Allow and Deny report.
    public var id: String
    /// The session it belongs to, which a click on a question opens.
    public var session: String
    public var title: String
    public var kind: NeedKind

    public init(id: String, session: String, title: String, kind: NeedKind) {
      (self.id, self.session, self.title, self.kind) = (id, session, title, kind)
    }
  }

  /// One session with a turn running.
  public struct Running: Identifiable, Equatable, Sendable {
    /// The session's id, which a click opens.
    public var id: String
    public var title: String
    /// What it does now, how long the turn has run and what it cost so far: `cargo nextest`,
    /// `12 s`, `$0.42`.
    public var activity: String
    public var elapsed: String
    public var cost: String

    public init(id: String, title: String, activity: String, elapsed: String, cost: String) {
      (self.id, self.title, self.activity, self.elapsed, self.cost) =
        (id, title, activity, elapsed, cost)
    }
  }

  public enum Intent: Equatable, Sendable {
    case allow(Need.ID)
    case deny(Need.ID)
    /// Opens the app on a session: a question's, or a running one's.
    case open(session: String)
    case newSession
    case openApp
  }

  let state: State
  let send: (Intent) -> Void

  /// Mockup 26's popover: `width: 360px`.
  static let width: CGFloat = 360

  public init(state: State, send: @escaping (Intent) -> Void) {
    self.state = state
    self.send = send
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.xs) {
      if !state.needs.isEmpty {
        SectionHeader("Needs you").padding(.horizontal, Space.m)
        ForEach(state.needs) { need($0) }
        Hairline().padding(.vertical, Space.xs)
      }
      if !state.running.isEmpty {
        SectionHeader("Running").padding(.horizontal, Space.m)
        ForEach(state.running) { running($0) }
        Hairline().padding(.vertical, Space.xs)
      }
      if state.needs.isEmpty && state.running.isEmpty {
        Text("Nothing needs you")
          .textStyle(.footnote)
          .foregroundStyle(Color(.textSecondary))
          .padding(.horizontal, Space.m)
          .padding(.vertical, Space.s)
        Hairline().padding(.vertical, Space.xs)
      }
      footer
    }
    .padding(Space.s)
    .frame(width: Self.width, alignment: .leading)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("cox")
  }

  @ViewBuilder private func need(_ need: Need) -> some View {
    switch need.kind {
    case .approval(let command):
      HStack(alignment: .top, spacing: Space.m) {
        StatusDot(.waiting).padding(.top, Space.xs)
        lines(need.title) {
          Text(command).textStyle(.monoInline).lineLimit(1).truncationMode(.middle)
        }
        Spacer(minLength: Space.s)
        HStack(spacing: Space.xs) {
          Button("Allow") { send(.allow(need.id)) }
            .buttonStyle(CoxButtonStyle(.primary, size: .small))
          Button("Deny") { send(.deny(need.id)) }
            .buttonStyle(CoxButtonStyle(.secondary, size: .small))
        }
      }
      .modifier(MenuRow())
    case .question(let asks):
      Button {
        send(.open(session: need.session))
      } label: {
        HStack(alignment: .center, spacing: Space.m) {
          StatusDot(.waiting)
          lines(need.title) { Text("asks: \(asks)").lineLimit(1) }
          Spacer(minLength: 0)
        }
        .modifier(MenuRow())
      }
      .buttonStyle(.plain)
    }
  }

  private func running(_ row: Running) -> some View {
    Button {
      send(.open(session: row.id))
    } label: {
      HStack(alignment: .center, spacing: Space.m) {
        Spinner()
        lines(row.title) {
          Text(
            [row.activity, row.elapsed, row.cost].filter { !$0.isEmpty }.joined(separator: " · ")
          )
          .textStyle(.footnote, tabularDigits: true)
          .lineLimit(1)
        }
        Spacer(minLength: 0)
      }
      .modifier(MenuRow())
    }
    .buttonStyle(.plain)
  }

  private var footer: some View {
    VStack(alignment: .leading, spacing: 0) {
      item("Today", trailing: state.today, action: nil)
      item("New session…", trailing: "⌥⌘N") { send(.newSession) }
      item("Open cox", trailing: "⌘O") { send(.openApp) }
    }
  }

  /// A title over its detail line.
  private func lines<Detail: View>(
    _ title: String, @ViewBuilder detail: () -> Detail
  ) -> some View {
    VStack(alignment: .leading, spacing: Space.xxs) {
      Text(title)
        .textStyle(.body)
        .fontWeight(.semibold)
        .foregroundStyle(Color(.textPrimary))
        .lineLimit(1)
      detail()
        .textStyle(.footnote)
        .foregroundStyle(Color(.textSecondary))
    }
  }

  /// A menu line: a label with its figures or shortcut at the trailing edge; a button when it
  /// has an action.
  @ViewBuilder private func item(
    _ label: String, trailing: String, action: (() -> Void)?
  ) -> some View {
    let line = HStack(spacing: Space.m) {
      Text(label).foregroundStyle(Color(.textPrimary))
      Spacer(minLength: Space.m)
      Text(trailing).foregroundStyle(Color(.textSecondary))
    }
    .textStyle(.body, tabularDigits: true)
    .modifier(MenuRow())
    if let action {
      Button(action: action) { line }.buttonStyle(.plain)
    } else {
      line
    }
  }
}

/// A row's inset and hit area, as the mockup's `.it`.
private struct MenuRow: ViewModifier {
  func body(content: Content) -> some View {
    content
      .padding(.horizontal, Space.m)
      .padding(.vertical, Space.s)
      .frame(maxWidth: .infinity, alignment: .leading)
      .contentShape(Rectangle())
  }
}

#Preview("empty") {
  PreviewMatrix { MenuBarPanel(state: PreviewState.menuBarEmpty) { _ in } }
}
#Preview("busy") {
  PreviewMatrix { MenuBarPanel(state: PreviewState.menuBarBusy) { _ in } }
}
