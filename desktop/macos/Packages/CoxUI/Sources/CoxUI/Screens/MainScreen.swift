// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `MainScreen` (DS§4, DS§6.5; DT§5.1): the window shell — the sidebar, the toolbar, the
// transcript column and the inspector as floating panes on the window's glass. Composition
// only (DS§5): the panes style themselves, and the transcript and inspector tabs are slots
// their own cards fill (T37.23, T37.24, T37.29). The app binds `state` and `send` to its stores
// and fills the sidebar's and toolbar's values, which are public for it (T37.22.5).

import AppKit
import SwiftUI

/// What the shell shows: each pane's state and which panes are out.
public struct MainScreenState: Equatable, Sendable {
  public var sidebar = Sidebar.State()
  public var toolbar = SessionToolbar.State()
  public var isSidebarVisible = true
  public var isInspectorVisible = true
  public var inspectorTab = InspectorTab.changes
  /// Shown while the toolbar's `popover` is `.appearance`.
  public var appearance = AppearancePopover.State()
  /// Shown while the toolbar's `popover` is `.model`.
  public var model = ModelPopover.State()

  public init() {}

  init(
    sidebar: Sidebar.State, toolbar: SessionToolbar.State, isSidebarVisible: Bool = true,
    isInspectorVisible: Bool = true
  ) {
    (self.sidebar, self.toolbar) = (sidebar, toolbar)
    (self.isSidebarVisible, self.isInspectorVisible) = (isSidebarVisible, isInspectorVisible)
  }
}

extension MainScreenState {
  /// The toolbar popover that is open; the one open marks its capsule active.
  public var popover: SessionToolbar.Popover? {
    get { toolbar.popover }
    set { toolbar.popover = newValue }
  }
}

/// Every intent the shell reports, tagged by the pane it came from.
public enum MainScreenIntent: Equatable, Sendable {
  case sidebar(Sidebar.Intent)
  case toolbar(SessionToolbar.Intent)
  case inspectorTab(InspectorTab)
  case appearance(AppearancePopover.Intent)
  /// A model popover row's id.
  case model(String)
  /// A click outside the open popover, or Esc.
  case dismissPopover
}

/// The shell for one open session. The sidebar and inspector fold away as `state` says; the
/// material, window opacity and Depth reach every pane from `coxAppearance`.
public struct MainScreen<Transcript: View, InspectorContent: View>: View {
  let state: MainScreenState
  let send: (MainScreenIntent) -> Void
  let transcript: Transcript
  let inspector: (InspectorTab) -> InspectorContent

  public init(
    state: MainScreenState, send: @escaping (MainScreenIntent) -> Void,
    @ViewBuilder transcript: () -> Transcript,
    @ViewBuilder inspector: @escaping (InspectorTab) -> InspectorContent
  ) {
    self.state = state
    self.send = send
    self.transcript = transcript()
    self.inspector = inspector
  }

  /// Below this window width the inspector floats over the transcript column instead of taking
  /// width from it, so a small window keeps its reading room (DS§4).
  static var inspectorFloatsBelow: CGFloat { 1280 }

  /// Esc's key code, which `WindowKeys` sees before the focused editor takes it.
  private static var escape: UInt16 { 53 }

  public var body: some View {
    GeometryReader { window in
      shell(inspectorFloats: window.size.width < Self.inspectorFloatsBelow)
    }
    .overlay {
      if state.toolbar.popover != nil {
        // A transient popover's dismissal (HIG): a click anywhere outside it, or Esc.
        Color.clear.contentShape(Rectangle())
          .onTapGesture { send(.dismissPopover) }
          .background(WindowKeys(handle: dismissOnEscape))
          .accessibilityHidden(true)
      }
    }
    .overlay(alignment: .topTrailing) {
      if state.toolbar.popover == .appearance {
        // Under the toolbar's Appearance button: past the inspector button and the bar's
        // trailing inset, as the mockup's `.appear` sits.
        AppearancePopover(state: state.appearance) { send(.appearance($0)) }
          .padding(.top, Size.toolbarHeight)
          .padding(.trailing, Space.l + Size.capsuleHeight)
      }
    }
    .overlayPreferenceValue(ModelCapsuleAnchor.self) { anchor in
      if state.toolbar.popover == .model, let anchor {
        GeometryReader { window in
          // Under the capsule, its leading edges aligned, kept inside the window.
          let capsule = window[anchor]
          ModelPopover(state: state.model) { send(.model($0)) }
            .offset(
              x: min(capsule.minX, window.size.width - Size.popoverWidth - Space.l),
              y: capsule.maxY + Space.xs)
        }
      }
    }
    .frame(minWidth: Size.windowMinWidth, minHeight: Size.windowMinHeight)
    .animation(.cox(Motion.durationSlow), value: state.isSidebarVisible)
    .animation(.cox(Motion.durationSlow), value: state.isInspectorVisible)
  }

  private func dismissOnEscape(_ event: NSEvent) -> Bool {
    guard event.keyCode == Self.escape else { return false }
    send(.dismissPopover)
    return true
  }

  private func shell(inspectorFloats: Bool) -> some View {
    ShellPane(.window) {
      HStack(spacing: Size.paneGap) {
        if state.isSidebarVisible {
          Sidebar(state: state.sidebar) { send(.sidebar($0)) }
            .padding([.leading, .vertical], Size.paneGap)
        }
        VStack(spacing: 0) {
          SessionToolbar(
            state: state.toolbar, isSidebarVisible: state.isSidebarVisible,
            isInspectorVisible: state.isInspectorVisible
          ) { send(.toolbar($0)) }
          HStack(spacing: Size.paneGap) {
            ShellPane(.column) { transcript.frame(maxWidth: .infinity, maxHeight: .infinity) }
            if state.isInspectorVisible, !inspectorFloats { inspectorPane }
          }
          .overlay(alignment: .trailing) {
            if state.isInspectorVisible, inspectorFloats {
              inspectorPane.padding([.vertical, .trailing], Size.paneGap)
            }
          }
          .padding([.trailing, .bottom], Size.paneGap)
          .padding(.leading, state.isSidebarVisible ? 0 : Size.paneGap)
        }
      }
    }
  }

  private var inspectorPane: some View {
    Inspector(selection: state.inspectorTab, content: inspector(state.inspectorTab)) {
      send(.inspectorTab($0))
    }
  }
}

#Preview("main") {
  MainScreenSample(state: PreviewState.main)
    .frame(width: PreviewState.window.width, height: PreviewState.window.height)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}
#Preview("panes folded") {
  MainScreenSample(state: PreviewState.folded)
    .frame(width: PreviewState.window.width, height: PreviewState.window.height)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}
