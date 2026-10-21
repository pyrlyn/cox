// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Install Command-Line Tool (DT§7, T37.32.2): a symlink from `~/.local/bin/cox` to the CLI
// inside the bundle, so the terminal's `cox` is the app's own build and the two never drift in
// version or database schema. `~/.local/bin` because install.sh puts `cox` there and it needs no
// administrator; a real file there is someone's install.sh copy and is never replaced.

import Foundation

public enum CommandLineTool {
  public enum Failure: Error, Equatable {
    /// The bundle has no CLI: a Debug build, which leaves it out.
    case notBundled(URL)
    /// Something other than a symlink already sits at the path.
    case occupied(URL)
  }

  /// Where the release build puts the CLI (`scripts/desktop/app.sh`).
  public static func bundled(in app: URL) -> URL {
    app.appending(path: "Contents/Helpers/cox", directoryHint: .notDirectory)
  }

  public static var defaultDirectory: URL {
    FileManager.default.homeDirectoryForCurrentUser.appending(
      path: ".local/bin", directoryHint: .isDirectory)
  }

  /// Links `directory/cox` to `tool` and returns the link. An older link — to a moved or
  /// replaced app — is pointed at `tool`.
  @discardableResult
  public static func link(
    _ tool: URL, into directory: URL = defaultDirectory, fileManager: FileManager = .default
  ) throws -> URL {
    guard fileManager.isExecutableFile(atPath: tool.path) else { throw Failure.notBundled(tool) }
    let link = directory.appending(path: "cox", directoryHint: .notDirectory)
    if let target = try? fileManager.destinationOfSymbolicLink(atPath: link.path) {
      if target == tool.path { return link }
      try fileManager.removeItem(at: link)
    } else if fileManager.fileExists(atPath: link.path) {
      throw Failure.occupied(link)
    }
    try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
    try fileManager.createSymbolicLink(atPath: link.path, withDestinationPath: tool.path)
    return link
  }
}
