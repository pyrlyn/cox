// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Which core the app talks to (DT§4.1, §8): the live Rust core, or a recorded patch stream from
// `desktop/macos/Fixtures` replayed by `FixtureCoreClient`, so the whole app runs without Rust
// state or a key. Settings and the first-run checklist always read the live core, since a
// recording has neither. Separate from the window so the one launch-time choice has one owner.
// The one browser page the agent drives (T51.9) is made here too, so the host the core calls and
// the pane the person watches (T51.10) hold the same page.
//
//   Cox.app/Contents/MacOS/Cox -CoxFixture desktop/macos/Fixtures/edit.json
//   COX_HOME=/tmp/cox-scratch Cox.app/Contents/MacOS/Cox -CoxProject ~/src/repo

import CoxClient
import CoxCore
import CoxPlatform
import Foundation

struct LaunchCore {
  /// Where sessions open: the fixture's replay or `live`.
  let core: Result<any CoreClient, any Error>
  /// The Rust core at `COX_HOME`, `~/.cox` when unset: settings and the checklist.
  let live: Result<LiveCoreClient, any Error>
  /// Where provider keys live; the host's `secret` and Settings' key fields read the same one.
  let secrets: any SecretStore
  let isFixture: Bool
  /// The page the agent's browser tools drive and the browser pane shows.
  let browser: BrowserController

  /// `-CoxFixture <path>` (a launch argument, read through `UserDefaults`' argument domain)
  /// replays that fixture; otherwise sessions open on the live core. `COX_KEYRING=off`, as
  /// cargo sets it for every development run, keeps provider keys out of the Keychain here too
  /// (A49, A51): a dev launch never raises a Keychain prompt.
  @MainActor
  static func pick(
    _ defaults: UserDefaults = .standard,
    environment: [String: String] = ProcessInfo.processInfo.environment
  ) -> LaunchCore {
    let secrets: any SecretStore =
      environment["COX_KEYRING"] == "off" ? MemorySecretStore() : KeychainSecretStore()
    let browser = BrowserController()
    let live = Result {
      try LiveCoreClient(
        home: environment["COX_HOME"],
        host: HostBridge(MacHost(secrets: secrets, browser: browser)))
    }
    let fixture = defaults.string(forKey: "CoxFixture")
    let core: Result<any CoreClient, any Error> =
      if let fixture {
        Result { FixtureCoreClient(fixture: try Fixture(contentsOf: URL(filePath: fixture))) }
      } else {
        live.map { $0 }
      }
    return LaunchCore(
      core: core, live: live, secrets: secrets, isFixture: fixture != nil, browser: browser)
  }

  /// The directory a new session works in: `-CoxProject <path>` or the folder first run chose,
  /// else the home directory (a Finder launch's working directory is `/`).
  static func project(_ defaults: UserDefaults = .standard) -> String {
    defaults.string(forKey: projectKey) ?? NSHomeDirectory()
  }

  static let projectKey = "CoxProject"
}
