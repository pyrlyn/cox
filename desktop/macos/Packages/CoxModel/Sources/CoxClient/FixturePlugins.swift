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

extension FixtureSession {
  public func pluginTabs() -> [PluginTab] { plugins.tabs }
  public func pluginToolbar() -> [PluginAction] { plugins.toolbar }

  public func selectPluginTab(_ plugin: String?, width: UInt16, height: UInt16) {
    plugins.select(plugin)
  }

  public func pluginAction(_ id: String) async -> Bool { plugins.act(id) }
}
