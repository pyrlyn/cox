// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Sidebar` (DS§6.4 row `Sidebar`, the mockup's `.sidebar`; DT§5.1): the session list — the
// filter, the status sections ("Needs you", "Running"), the projects as disclosure groups, and
// the footer with New session and the providers' health. A "Needs you" row is one inbox item
// (T37.27.7), which opens its session and, once expired, is read-only. A row's context menu
// renames its session (A113) and opens it in a new window or tab (T51.11). A remote host's
// sessions are a group of their own under a host badge, disconnected with a Reconnect action once
// its connection dropped (T52.21). Separate so the window shell shows sessions from one value the
// core fills, and reports what the person does as intents.

import SwiftUI

/// The session list on a floating `ShellPane(.sidebar)` of `Size.sidebarWidth`. It renders
/// `state` and reports every action through `send`; it owns no session state.
public struct Sidebar: View {
  public struct Session: Equatable, Sendable, Identifiable {
    /// The session's stable id from Rust, or an inbox item's: one session can wait on several.
    public let id: String
    public var row: SessionRow.Item
    /// The session an inbox item's row opens; `nil` when `id` is the session.
    public var session: ID?
    /// An expired inbox item: shown, but no longer answerable from this window.
    public var isReadOnly: Bool

    var opens: ID { session ?? id }

    public init(id: String, row: SessionRow.Item, session: ID? = nil, isReadOnly: Bool = false) {
      (self.id, self.row, self.session, self.isReadOnly) = (id, row, session, isReadOnly)
    }
  }

  public struct Group: Equatable, Sendable, Identifiable {
    public let id: String
    public var title: String
    public var kind: Kind
    public var sessions: [Session]

    public init(id: String, title: String, kind: Kind, sessions: [Session]) {
      (self.id, self.title, self.kind, self.sessions) = (id, title, kind, sessions)
    }
  }

  public struct State: Equatable, Sendable {
    public var filter: String
    public var groups: [Group]
    public var selection: Session.ID?
    /// The providers' health as the core formats it, `3 providers`, and its dot.
    public var providers: String
    public var providerStatus: StatusDot.Status

    public init(
      filter: String = "", groups: [Group] = [], selection: Session.ID? = nil,
      providers: String = "", providerStatus: StatusDot.Status = .idle
    ) {
      (self.filter, self.groups, self.selection) = (filter, groups, selection)
      (self.providers, self.providerStatus) = (providers, providerStatus)
    }
  }

  public enum Intent: Equatable, Sendable {
    case filter(String)
    case select(Session.ID)
    case toggle(Group.ID)
    case newSession
    case hide
    /// The session and the title typed for it in the row's Rename… sheet (A113).
    case rename(Session.ID, String)
    /// The row's Open in New Window or Open in New Tab (T51.11).
    case popOut(Session.ID, asTab: Bool)
    /// A disconnected host group's Reconnect (T52.21).
    case reconnect(Group.ID)
  }

  let state: State
  let send: (Intent) -> Void
  /// The row whose Rename… alert is open.
  @SwiftUI.State private var renaming: Session?
  @SwiftUI.State private var draft = ""

  init(state: State, send: @escaping (Intent) -> Void) {
    self.state = state
    self.send = send
  }

  public var body: some View {
    ShellPane(.sidebar) {
      VStack(spacing: 0) {
        HStack {
          Spacer()
          Button {
            send(.hide)
          } label: {
            Image(systemName: "sidebar.left").symbolStyle(.transcriptH3)
          }
          .buttonStyle(CoxButtonStyle(.plain, size: .small))
          .foregroundStyle(Color(.textSecondary))
          .keyboardShortcut(ShellShortcut.sidebar.key)
          .help(ShellShortcut.sidebar.help("Hide the sidebar"))
          .accessibilityLabel("Hide sidebar")
        }
        // The system's window buttons sit at the leading end of this row, beside the toolbar.
        .padding(.horizontal, Space.xs)
        .frame(height: Size.toolbarHeight - Size.paneGap)
        SessionFilter(
          text: Binding(get: { state.filter }, set: { send(.filter($0)) }),
          prompt: "Filter sessions", shortcut: "⌘K"
        )
        .padding(.horizontal, Space.l)
        .padding(.bottom, Space.ml)
        ScrollView {
          LazyVStack(alignment: .leading, spacing: Space.xxs) {
            ForEach(state.groups) {
              SidebarGroup(group: $0, selection: state.selection, send: send) { session in
                (renaming, draft) = (session, session.row.title)
              }
            }
          }
          .padding(.bottom, Space.m)
        }
        SidebarFooter(providers: state.providers, status: state.providerStatus) {
          send(.newSession)
        }
      }
    }
    .frame(width: Size.sidebarWidth)
    .coxTransition(.move(edge: .leading).combined(with: .opacity))
    .alert("Rename session", isPresented: isRenaming) {
      TextField("Title", text: $draft)
      Button("Rename") {
        let title = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if let renaming, !title.isEmpty, title != renaming.row.title {
          send(.rename(renaming.opens, title))
        }
      }
      Button("Cancel", role: .cancel) {}
    }
  }

  private var isRenaming: Binding<Bool> {
    Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
  }
}

extension Sidebar.Group {
  public enum Kind: Equatable, Sendable {
    /// A status section with the count the core formats, or `nil` for none.
    case section(count: String?)
    /// A project, open or folded.
    case project(isExpanded: Bool)
    /// A remote host's sessions (T52.21), and whether its connection holds.
    case host(isConnected: Bool)
  }
}

/// One section or project: its header, then its sessions unless the project is folded.
private struct SidebarGroup: View {
  let group: Sidebar.Group
  let selection: Sidebar.Session.ID?
  let send: (Sidebar.Intent) -> Void
  /// Opens the Rename… alert for a row.
  let rename: (Sidebar.Session) -> Void

  var body: some View {
    switch group.kind {
    case .section(let count):
      SectionHeader(group.title) { if let count { CountBadge(count) } }
        // Level with the row text: the row's inset plus its own padding.
        .padding(.horizontal, Space.m + Space.ml)
        .padding(.top, Space.ml)
        .padding(.bottom, Space.xs)
      rows
    case .project(let isExpanded):
      Button {
        send(.toggle(group.id))
      } label: {
        HStack(spacing: Space.s) {
          Image(systemName: isExpanded ? "chevron.down" : "chevron.right").symbolStyle(.micro)
          Text(group.title).textStyle(.control)
        }
        .foregroundStyle(Color(.textSecondary))
        .padding(.horizontal, Space.xl)
        .padding(.top, Space.s)
        .padding(.bottom, Space.xxs)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityValue(isExpanded ? "Expanded" : "Collapsed")
      if isExpanded { rows }
    case .host(let isConnected):
      HostHeader(host: group.title, isConnected: isConnected) { send(.reconnect(group.id)) }
      rows
    }
  }

  private var rows: some View {
    ForEach(group.sessions) { row($0).padding(.horizontal, Space.m) }
  }

  @ViewBuilder private func row(_ session: Sidebar.Session) -> some View {
    let row = SessionRow(session.row, isSelected: session.opens == selection)
    if session.isReadOnly {
      // Not a disabled button: `.plain` would fade its label below DS§8. The row answers
      // nothing and its title steps down to `text.secondary`, as a disabled control's does.
      row.disabled(true)
    } else {
      Button {
        send(.select(session.opens))
      } label: {
        row
      }
      .buttonStyle(.plain)
      .contextMenu {
        // A session's own row; an inbox row's text is the item's, not the session's title.
        if session.session == nil { Button("Rename…") { rename(session) } }
        Button("Open in New Window") { send(.popOut(session.opens, asTab: false)) }
        Button("Open in New Tab") { send(.popOut(session.opens, asTab: true)) }
      }
    }
  }
}

/// A remote host's header: the host badge — a server glyph and the `ssh` alias in a well — then,
/// once the connection dropped, a failed dot, "Disconnected" and Reconnect. Its rows are read-only
/// meanwhile; the core marks them so.
struct HostHeader: View {
  let host: String
  let isConnected: Bool
  let reconnect: () -> Void

  var body: some View {
    HStack(spacing: Space.s) {
      HStack(spacing: Space.xs) {
        Image(systemName: "server.rack").symbolStyle(.micro)
        Text(host).textStyle(.control).lineLimit(1).truncationMode(.middle)
      }
      .foregroundStyle(Color(.textSecondary))
      .padding(.horizontal, Space.s)
      .padding(.vertical, Space.xxs)
      .insetWell(Color(.fillPrimary), cornerRadius: Radius.s)
      .accessibilityElement(children: .combine)
      .accessibilityLabel("Host \(host)")
      if isConnected {
        Spacer(minLength: 0)
      } else {
        StatusDot(.error)
        Text("Disconnected").textStyle(.caption).foregroundStyle(Color(.textSecondary))
          .lineLimit(1)
        Spacer(minLength: Space.s)
        Button("Reconnect", action: reconnect)
          .buttonStyle(CoxButtonStyle(.plain, size: .small))
      }
    }
    .padding(.leading, Space.xl)
    .padding(.trailing, Space.l)
    .padding(.top, Space.s)
    .padding(.bottom, Space.xxs)
  }
}

/// New session at the leading end, the providers' health at the trailing end, under a hairline.
private struct SidebarFooter: View {
  let providers: String
  let status: StatusDot.Status
  let newSession: () -> Void

  var body: some View {
    HStack(spacing: Space.xs) {
      Button(action: newSession) {
        HStack(spacing: Space.s) {
          Image(systemName: "plus").symbolStyle(.body)
          Text("New session")
          KeyCap("⌘N")
        }
      }
      .buttonStyle(CoxButtonStyle(.plain, size: .small))
      .layoutPriority(1)
      Spacer(minLength: 0)
      HStack(spacing: Space.s) {
        StatusDot(status)
        Text(providers).textStyle(.caption).foregroundStyle(Color(.textSecondary))
      }
      .lineLimit(1)
      .accessibilityElement(children: .combine)
    }
    .padding(.trailing, Space.l)
    .padding(.vertical, Space.m)
    .hairline(.top)
  }
}

#Preview("main") {
  PreviewMatrix {
    Sidebar(state: PreviewState.sidebar) { _ in }.frame(height: Size.windowMinHeight)
  }
}
#Preview("hosts") {
  PreviewMatrix {
    Sidebar(state: PreviewState.hostSidebar) { _ in }.frame(height: Size.windowMinHeight)
  }
}
#Preview("needs you") {
  PreviewMatrix {
    Sidebar(state: PreviewState.inboxSidebar) { _ in }.frame(height: Size.windowMinHeight)
  }
}
