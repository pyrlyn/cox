// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SessionToolbar` (DS§6.4 row `SessionToolbar`, the mockup's `.toolbar`; DT§5.1): the open
// session's bar above the transcript — where it lives, its title renamed by a double-click
// (A113), its model, mode and cost, Stop while a turn runs, the Appearance and inspector buttons
// with their shortcuts, and the Bypass strip.
// Separate so the window shell shows the session from one value the core fills, and reports what
// the person does as intents.

import SwiftUI

/// `Breadcrumb` at the leading end; the capsules, Stop and the icon buttons at the trailing
/// end, `Size.toolbarHeight` tall on the window's own glass. It owns no session state. While
/// the mode is Bypass a `status.danger` strip runs under the whole bar (DS§3.1, A89).
public struct SessionToolbar: View {
  /// The popover a capsule or button opens; the one open marks its capsule active.
  public enum Popover: Equatable, Sendable {
    case model, cost, appearance
  }

  public struct State: Equatable, Sendable {
    public var title: String
    public var project: String
    public var branch: String?
    /// The model and effort as the core formats them, `Sonnet 5 · high`.
    public var model: String
    public var mode: SessionMode
    /// What the core formatted: `$0.42` and `38%`, and the share the ring fills.
    public var cost: String
    public var context: String
    public var contextFraction: Double
    public var isRunning: Bool
    public var popover: Popover?
    /// The plugins' `status.left` then `status.right` segments (PL§8, T52.17): the first thing
    /// the bar drops when it runs out of room.
    public var pluginStatus: [PluginWidget]

    public init(
      title: String = "", project: String = "", branch: String? = nil, model: String = "",
      mode: SessionMode = .ask, cost: String = "", context: String = "",
      contextFraction: Double = 0, isRunning: Bool = false, popover: Popover? = nil,
      pluginStatus: [PluginWidget] = []
    ) {
      (self.title, self.project, self.branch, self.model, self.mode) =
        (title, project, branch, model, mode)
      (self.cost, self.context, self.contextFraction) = (cost, context, contextFraction)
      (self.isRunning, self.popover, self.pluginStatus) = (isRunning, popover, pluginStatus)
    }
  }

  public enum Intent: Equatable, Sendable {
    case showSidebar
    case open(Popover)
    case mode(SessionMode)
    case stop
    case toggleInspector
    /// The title the person typed after double-clicking it (A113).
    case rename(String)
  }

  let state: State
  /// The window's layout, which the screen owns: with the sidebar hidden its toggle moves here.
  let isSidebarVisible: Bool
  let isInspectorVisible: Bool
  let send: (Intent) -> Void

  init(
    state: State, isSidebarVisible: Bool = true, isInspectorVisible: Bool = true,
    send: @escaping (Intent) -> Void
  ) {
    self.state = state
    self.isSidebarVisible = isSidebarVisible
    self.isInspectorVisible = isInspectorVisible
    self.send = send
  }

  /// The system's close, minimise and zoom buttons at the window's leading edge, which the
  /// bar leaves clear while the sidebar that otherwise holds them is hidden: the mockup's
  /// `.traffic` row, 16 pt inset plus three 12 pt buttons 8 pt apart.
  private static let windowButtonsWidth: CGFloat = 68

  /// DS§3.1's Bypass strip: thin enough not to crowd the bar, thick enough to see at a glance.
  private static let bypassStripHeight: CGFloat = 3

  public var body: some View {
    HStack(spacing: Space.ml) {
      if !isSidebarVisible {
        Spacer().frame(width: Self.windowButtonsWidth)
        ToolbarIconButton(symbol: "sidebar.left", label: "Show sidebar", shortcut: .sidebar) {
          send(.showSidebar)
        }
      }
      Breadcrumb(state.title, project: state.project, branch: state.branch) {
        send(.rename($0))
      }
      Spacer(minLength: Space.ml)
      if !state.pluginStatus.isEmpty {
        // `ViewThatFits` falls back to nothing, so the segments go before any capsule does.
        ViewThatFits(in: .horizontal) {
          PluginStatusSegments(widgets: state.pluginStatus)
          Color.clear.frame(width: 0, height: 0)
        }
      }
      ModelCapsule(state.model, isOpen: state.popover == .model) { send(.open(.model)) }
        .anchorPreference(key: ModelCapsuleAnchor.self, value: .bounds) { $0 }
      ModeSegmented(selection: Binding(get: { state.mode }, set: { send(.mode($0)) }))
      CostCapsule(
        cost: state.cost, context: state.context, fraction: state.contextFraction,
        isOpen: state.popover == .cost
      ) { send(.open(.cost)) }
      if state.isRunning { StopButton { send(.stop) } }
      ToolbarIconButton(
        symbol: "paintbrush", label: "Appearance", shortcut: .appearance,
        isActive: state.popover == .appearance
      ) { send(.open(.appearance)) }
      ToolbarIconButton(
        symbol: "sidebar.right",
        label: isInspectorVisible ? "Hide inspector" : "Show inspector", shortcut: .inspector
      ) { send(.toggleInspector) }
    }
    .padding(.leading, Space.xl)
    .padding(.trailing, Space.toolbarTrailing)
    .frame(height: Size.toolbarHeight)
    .overlay(alignment: .bottom) {
      if state.mode == .bypass {
        // Along the panes below: the window's edge while the sidebar is out, a gap in from it.
        Capsule()
          .fill(Color(.statusDanger))
          .frame(height: Self.bypassStripHeight)
          .padding(.leading, isSidebarVisible ? 0 : Size.paneGap)
          .padding(.trailing, Size.paneGap)
          .accessibilityHidden(true)
      }
    }
  }
}

/// The plugins' status segments in a row, each at most PL§8's `SEGMENT_COLS` cells of the code
/// face wide and one line tall, truncated past that.
struct PluginStatusSegments: View {
  let widgets: [PluginWidget]

  /// PL§8's `SEGMENT_COLS`, in the code face's advance, which is about 0.6 of its size.
  private static let segmentWidth: CGFloat = FontToken.monoCode.size * 0.6 * 24

  var body: some View {
    HStack(spacing: Space.ml) {
      ForEach(Array(widgets.enumerated()), id: \.offset) { _, widget in
        PluginWidgetView(widget)
          .lineLimit(1)
          .fixedSize(horizontal: false, vertical: true)
          .frame(maxWidth: Self.segmentWidth, alignment: .leading)
          .clipped()
      }
    }
    .accessibilityElement(children: .combine)
    .accessibilityLabel("Plugin status")
  }
}

/// A shell toggle's key and the glyphs a tooltip names it by. The sidebar and inspector keys are
/// the defaults of the system `SidebarCommands` (⌃⌘S) and `InspectorCommands` (⌃⌘I), so the
/// app's View menu items and these buttons answer the same keys (A89); Appearance is DS§3.5's.
public struct ShellShortcut: Sendable {
  public let key: KeyboardShortcut
  public let glyphs: String

  public static let sidebar = Self(
    key: .init("s", modifiers: [.control, .command]), glyphs: "⌃⌘S")
  public static let inspector = Self(
    key: .init("i", modifiers: [.control, .command]), glyphs: "⌃⌘I")
  public static let appearance = Self(
    key: .init("a", modifiers: [.command, .option]), glyphs: "⌘⌥A")

  /// The tooltip for a control labelled `label`: its name, then its keys (DS§8).
  func help(_ label: String) -> String { "\(label) (\(glyphs))" }
}

/// A symbol alone on a round `CapsuleStyle` capsule, answering its shortcut, with its name as
/// label and its name and keys as tooltip (DS§8).
private struct ToolbarIconButton: View {
  let symbol: String
  let label: String
  let shortcut: ShellShortcut
  var isActive = false
  let action: () -> Void

  var body: some View {
    Button(action: action) { Image(systemName: symbol).symbolStyle(.transcriptH3) }
      .buttonStyle(CapsuleStyle(isActive ? .active : .plain, isIcon: true))
      .keyboardShortcut(shortcut.key)
      .help(shortcut.help(label))
      .accessibilityLabel(label)
  }
}

#Preview("running") {
  PreviewMatrix { SessionToolbar(state: PreviewState.toolbar) { _ in }.fixedSize() }
}
#Preview("sidebar hidden") {
  PreviewMatrix {
    SessionToolbar(state: PreviewState.toolbar, isSidebarVisible: false) { _ in }.fixedSize()
  }
}
