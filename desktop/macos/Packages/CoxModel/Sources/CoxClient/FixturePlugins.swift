// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The desktop plugin contributions (PL§15.4, T33.45.7) for a client without a core, and a
// fixture's declared ones. Separate from `CoreClient.swift`, which holds the protocol and the
// rest of the fixture, to keep that file within SwiftLint's length limit.

import Synchronization

extension SessionClient {
  /// A client without a core has no desktop contributions.
  public func pluginTabs() -> [PluginTab] { [] }
  public func pluginToolbar() -> [PluginAction] { [] }
  public func selectPluginTab(_ plugin: String?, width: UInt16, height: UInt16) {}
  public func pluginAction(_ id: String) async -> Bool { false }
  public func openLink(_ link: String) -> LinkClick { .refused }
}

/// The inspector tabs and toolbar items a fixture's plugins declare, and each tab chosen (`nil`
/// for a built-in one) and action clicked, for a test to read back.
public final class FixturePlugins: Sendable {
  private let state = Mutex(State())
  /// The canvases, kept here so the fixture session's stored properties do not change.
  public let canvases = FixtureCanvases()

  private struct State {
    var tabs: [PluginTab] = []
    var toolbar: [PluginAction] = []
    var selected: [String?] = []
    var acted: [String] = []
  }

  public init() {}

  /// Set before a store reads them.
  public func declare(tabs: [PluginTab], toolbar: [PluginAction]) {
    state.withLock { ($0.tabs, $0.toolbar) = (tabs, toolbar) }
  }

  public var tabs: [PluginTab] { state.withLock { $0.tabs } }
  public var toolbar: [PluginAction] { state.withLock { $0.toolbar } }
  public var selected: [String?] { state.withLock { $0.selected } }
  public var acted: [String] { state.withLock { $0.acted } }

  func select(_ plugin: String?) { state.withLock { $0.selected.append(plugin) } }

  /// True for a declared toolbar item, as the core answers for a granted command.
  func act(_ id: String) -> Bool {
    state.withLock { state in
      state.acted.append(id)
      return state.toolbar.contains { $0.id == id }
    }
  }
}

/// The canvases a fixture's plugins declare (plugin-canvas §3, T33.47.8.1): each one's drawn
/// update, sent when it is shown, redraws a test queues, and what was shown, hidden and clicked,
/// for the test to read back.
public final class FixtureCanvases: Sendable {
  private let state = Mutex(State())

  private struct State {
    var canvases: [PluginCanvas] = []
    var drawn: [String: CanvasUpdate] = [:]
    var answers: [String: CanvasAttach] = [:]
    var pending: [CanvasUpdate] = []
    var waiting: CheckedContinuation<CanvasUpdate?, Never>?
    var closed = false
    var shown: [String] = []
    var hidden: [String] = []
    var selected: [[String]] = []
  }

  public init() {}

  /// Set before a store reads them: the canvases, the update each one's show sends, and the
  /// attachment each clicked value gives.
  public func declare(
    _ canvases: [PluginCanvas], drawn: [CanvasUpdate] = [], answers: [String: CanvasAttach] = [:]
  ) {
    state.withLock { state in
      state.canvases = canvases
      state.drawn = Dictionary(drawn.map { ($0.plugin, $0) }) { first, _ in first }
      state.answers = answers
    }
  }

  /// A redraw, as a plugin's own tool call or `Effects.redraw` causes one.
  public func queue(_ update: CanvasUpdate) {
    let waiting = state.withLock { state in
      guard let waiting = state.waiting else {
        state.pending.append(update)
        return CheckedContinuation<CanvasUpdate?, Never>?.none
      }
      state.waiting = nil
      return waiting
    }
    waiting?.resume(returning: update)
  }

  /// Ends the pull, as closing the session does.
  public func close() {
    let waiting = state.withLock { state in
      state.closed = true
      defer { state.waiting = nil }
      return state.waiting
    }
    waiting?.resume(returning: nil)
  }

  public var canvases: [PluginCanvas] { state.withLock { $0.canvases } }
  /// Each show's plugin, in order.
  public var shown: [String] { state.withLock { $0.shown } }
  public var hidden: [String] { state.withLock { $0.hidden } }
  /// Each click as `[plugin, frame, value]`.
  public var selected: [[String]] { state.withLock { $0.selected } }

  func show(_ plugin: String) {
    let drawn = state.withLock { state in
      state.shown.append(plugin)
      return state.drawn[plugin]
    }
    if let drawn { queue(drawn) }
  }

  func hide(_ plugin: String) { state.withLock { $0.hidden.append(plugin) } }

  func next() async -> CanvasUpdate? {
    await withCheckedContinuation { continuation in
      state.withLock { state in
        if !state.pending.isEmpty {
          continuation.resume(returning: state.pending.removeFirst())
        } else if state.closed {
          continuation.resume(returning: nil)
        } else {
          state.waiting = continuation
        }
      }
    }
  }

  func select(_ plugin: String, frame: String, value: String) -> CanvasAttach? {
    state.withLock { state in
      state.selected.append([plugin, frame, value])
      return state.answers[value]
    }
  }
}

extension FixtureSession: CanvasClient {
  public func pluginCanvases() async throws -> [PluginCanvas] { plugins.canvases.canvases }

  public func showCanvas(_ plugin: String, widthPt: UInt32, heightPt: UInt32) async throws {
    plugins.canvases.show(plugin)
  }

  public func hideCanvas(_ plugin: String) async throws { plugins.canvases.hide(plugin) }
  public func nextCanvas() async -> CanvasUpdate? { await plugins.canvases.next() }

  public func selectCanvas(
    _ plugin: String, frame: String, value: String
  ) async throws -> CanvasAttach? {
    plugins.canvases.select(plugin, frame: frame, value: value)
  }
}

extension FixtureSession {
  public func pluginTabs() -> [PluginTab] { plugins.tabs }
  public func pluginToolbar() -> [PluginAction] { plugins.toolbar }

  public func selectPluginTab(_ plugin: String?, width: UInt16, height: UInt16) {
    plugins.select(plugin)
  }

  public func pluginAction(_ id: String) async -> Bool { plugins.act(id) }
}
