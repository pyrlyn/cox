// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The app's registry of open sessions (DT§4.5, DT§4.6, T51.11): every window that shows one
// session — the main window, a popped-out window or a native tab — shares its one
// `SessionStore`, its `ComposerStore` and its one patch pull. The registry keeps them while any
// window holds the session and lets them go with the last one. Letting go stops the pull and
// the session's shells and closes the client. That is the core's own close, which leaves a
// running turn running, so closing a window never stops a turn. Separate from the windows
// because no single window owns a session any more.

import CoxClient
import Foundation

@MainActor
public final class AppStore {
  /// The stores every window showing one session shares.
  public struct Shared {
    public let store: SessionStore
    public let composer: ComposerStore
  }

  private struct Entry {
    let shared: Shared
    let pull: Task<Void, Never>
    /// The windows holding the session, by the id each window keeps for its life.
    var windows: Set<UUID>
  }

  private var entries: [String: Entry] = [:]

  public init() {}

  /// Joins `window` to a session another window already shows; `nil` when none does, and the
  /// caller opens the session on the core.
  public func join(_ session: String, window: UUID) -> Shared? {
    guard var entry = entries[session] else { return nil }
    entry.windows.insert(window)
    entries[session] = entry
    return entry.shared
  }

  /// Holds a session `window` just opened on the core. When another window opened the same
  /// session meanwhile, the new client is closed and `window` joins that one, so one session
  /// never has two pulls.
  public func adopt(_ client: any SessionClient, window: UUID) -> Shared {
    if let shared = join(client.id, window: window) {
      client.close()
      return shared
    }
    let store = SessionStore(session: client)
    let shared = Shared(store: store, composer: ComposerStore(session: store))
    entries[client.id] = Entry(
      shared: shared, pull: Task { await store.run() }, windows: [window])
    return shared
  }

  /// `window` stops showing `session`. The last window to go stops the pull, ends the session's
  /// shells and closes the client; the core keeps any running turn.
  public func release(_ session: String, window: UUID) {
    guard var entry = entries[session] else { return }
    entry.windows.remove(window)
    guard entry.windows.isEmpty else {
      entries[session] = entry
      return
    }
    entries[session] = nil
    entry.pull.cancel()
    entry.shared.store.closeTerminals()
    entry.shared.store.session.close()
  }

  /// Some window shows `session`.
  public func isOpen(_ session: String) -> Bool { entries[session] != nil }
}
