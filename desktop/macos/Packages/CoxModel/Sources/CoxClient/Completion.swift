// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One composer completion row (DT§5.3), as cox-ffi's `Completion`: what the
// `cox_app::Completer` offers for an `@` file or a `/` command. Separate from
// the timeline because it answers a call, not a patch.

public struct Completion: Equatable, Sendable {
  /// What replaces the typed token, `@src/lib.rs` or `/compact`.
  public var insert: String
  /// The path or the command's usage, for the row's second line.
  public var detail: String

  public init(insert: String, detail: String) {
    (self.insert, self.detail) = (insert, detail)
  }
}
