// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Updates through Sparkle 2 (DT§7, T37.32.2): the app menu's Check for Updates…, and the rule
// that an update never stops a running turn — with sessions running, Sparkle's relaunch waits
// until the person picks Relaunch Now, and otherwise installs when they quit. Separate from
// `CoxApp` because only a release build updates: the updater starts only when the bundle names
// the appcast's EdDSA public key (`COX_SPARKLE_PUBLIC_KEY`, passed by the release build), so a
// Debug, dev or fixture launch never checks a feed it could not verify.

import Combine
import Sparkle
import SwiftUI

@MainActor
final class Updates: NSObject, SPUUpdaterDelegate {
  /// The running sessions' count, asked when Sparkle is about to relaunch; the app model sets it.
  var running: () -> Int = { 0 }
  /// `nil` when this build has no public key, and then the menu item is absent.
  private(set) var controller: SPUStandardUpdaterController?

  override init() {
    super.init()
    guard Self.isConfigured(Bundle.main) else { return }
    controller = SPUStandardUpdaterController(
      startingUpdater: true, updaterDelegate: self, userDriverDelegate: nil)
  }

  /// Sparkle refuses to start without a public key and says so in an alert at every launch, so
  /// a build without one does not start it at all.
  static func isConfigured(_ bundle: Bundle) -> Bool {
    let key = bundle.object(forInfoDictionaryKey: "SUPublicEDKey") as? String ?? ""
    return !key.trimmingCharacters(in: .whitespaces).isEmpty
  }

  /// Relaunching ends the process, and with it every turn the core runs, so the person decides
  /// while any session is running; Install on Quit leaves the handler uncalled, and Sparkle's
  /// installer finishes the update when the app quits.
  func updater(
    _ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
    untilInvokingBlock installHandler: @escaping () -> Void
  ) -> Bool {
    let count = running()
    guard count > 0 else { return false }
    let alert = NSAlert()
    alert.messageText = "Relaunch Cox to finish the update?"
    alert.informativeText =
      count == 1
      ? "A session is running. Relaunching now stops its turn."
      : "\(count) sessions are running. Relaunching now stops their turns."
    alert.addButton(withTitle: "Install on Quit")
    alert.addButton(withTitle: "Relaunch Now")
    return alert.runModal() != .alertSecondButtonReturn
  }
}

/// Check for Updates… under About in the app menu, greyed while a check is under way.
struct UpdateCommands: Commands {
  let updates: Updates

  var body: some Commands {
    CommandGroup(after: .appInfo) {
      if let updater = updates.controller?.updater {
        CheckForUpdates(updater: updater)
      }
    }
  }
}

private struct CheckForUpdates: View {
  let updater: SPUUpdater
  @State private var canCheck = false

  var body: some View {
    Button("Check for Updates…") { updater.checkForUpdates() }
      .disabled(!canCheck)
      .onReceive(updater.publisher(for: \.canCheckForUpdates)) { canCheck = $0 }
  }
}
