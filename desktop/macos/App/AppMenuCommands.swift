// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The app menu's Check for Updates… and Install Command-Line Tool… under About (DT§7,
// T37.32.2). Check for Updates is off in a build with no update feed (Debug, local builds);
// the install links `~/.local/bin/cox` to the CLI inside this bundle and says what it did.

import AppKit
import CoxPlatform
import SwiftUI

struct AppMenuCommands: Commands {
  let updater: AppUpdater

  var body: some Commands {
    CommandGroup(after: .appInfo) {
      Button("Check for Updates…") { updater.checkForUpdates() }
        .disabled(!updater.isEnabled)
      Button("Install Command-Line Tool…") { Self.installCommandLineTool() }
    }
  }

  @MainActor
  private static func installCommandLineTool() {
    let alert = NSAlert()
    do {
      let link = try CommandLineTool.link(CommandLineTool.bundled(in: Bundle.main.bundleURL))
      alert.messageText = "Installed the cox command"
      alert.informativeText =
        "\(link.path) now runs this app's cox. If your shell does not find it, add "
        + "~/.local/bin to PATH."
    } catch CommandLineTool.Failure.notBundled {
      alert.messageText = "This build has no command-line tool"
      alert.informativeText = "Release builds carry cox inside the app; Debug builds do not."
    } catch CommandLineTool.Failure.occupied(let path) {
      alert.messageText = "cox is already installed"
      alert.informativeText =
        "\(path.path) is a file, not a link to this app, so it was left as it is. Remove it "
        + "and install again to use the app's cox."
    } catch {
      alert.messageText = "Could not install the cox command"
      alert.informativeText = error.localizedDescription
    }
    alert.runModal()
  }
}
