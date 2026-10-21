// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Updates outside the App Store (DT§7, T37.32.2) over Sparkle 2's standard updater. It starts
// only when Info.plist names an https feed and an EdDSA public key: a Debug or local build has
// no key, and Sparkle would otherwise show a misconfiguration alert on every launch. Updates
// download in the background and install on quit; a relaunch while sessions run asks first,
// because it would stop their turns (the Codex SIGKILL-on-update complaint, R9.2).

import AppKit
import Sparkle

/// The feed this build checks, read from its Info.plist.
public struct UpdateFeed: Equatable, Sendable {
  public let url: URL

  /// `nil` unless `SUFeedURL` is an https URL and `SUPublicEDKey` is set: an empty build setting
  /// expands to an empty string, and a feed without a key could not be trusted anyway.
  public static func configured(_ info: [String: Any]) -> UpdateFeed? {
    guard let feed = info["SUFeedURL"] as? String, let url = URL(string: feed),
      url.scheme == "https", url.host() != nil,
      let key = info["SUPublicEDKey"] as? String, !key.isEmpty
    else { return nil }
    return UpdateFeed(url: url)
  }
}

@MainActor
public final class AppUpdater: NSObject, SPUUpdaterDelegate {
  /// How many sessions are running a turn right now; the app sets it once its model exists.
  public var runningSessions: () -> Int = { 0 }
  private var controller: SPUStandardUpdaterController?

  public init(info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
    super.init()
    guard UpdateFeed.configured(info) != nil else { return }
    controller = SPUStandardUpdaterController(
      startingUpdater: true, updaterDelegate: self, userDriverDelegate: nil)
  }

  /// `false` for a build with no feed, so its menu item stays off.
  public var isEnabled: Bool { controller != nil }

  public func checkForUpdates() { controller?.checkForUpdates(nil) }

  public func updater(
    _ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
    untilInvokingBlock installHandler: @escaping () -> Void
  ) -> Bool {
    Self.postpones(
      running: runningSessions(), confirm: Self.confirmRelaunch, install: installHandler)
  }

  /// With nothing running the relaunch goes ahead. Otherwise it waits for a yes; on a no the
  /// downloaded update stays ready and installs when cox quits.
  static func postpones(
    running: Int, confirm: @escaping @MainActor (Int) -> Bool,
    install: @escaping () -> Void
  ) -> Bool {
    guard running > 0 else { return false }
    // After Sparkle's callback returns, so the alert is not modal inside it.
    Task { @MainActor in if confirm(running) { install() } }
    return true
  }

  private static func confirmRelaunch(_ running: Int) -> Bool {
    let alert = NSAlert()
    alert.messageText = "Relaunch to finish the update?"
    alert.informativeText =
      "\(running) session\(running == 1 ? " is" : "s are") running. Relaunching stops "
      + "\(running == 1 ? "its turn" : "their turns"). Later installs the update when you quit Cox."
    alert.addButton(withTitle: "Later")
    alert.addButton(withTitle: "Relaunch")
    return alert.runModal() == .alertSecondButtonReturn
  }
}
