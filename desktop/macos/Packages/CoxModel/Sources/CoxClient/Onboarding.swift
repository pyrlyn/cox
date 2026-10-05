// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The first-run checklist (DT§5.8, T37.31) as cox-ffi exports `cox_app::onboarding`: per check,
// how it went and the one line saying what is missing, and the `OnboardingClient` seam the app
// reads it through. Separate from Settings because the checklist is its own first-run window;
// `LiveCoreClient` (CoxCore) is the Rust side.

/// One checklist row, in the order Rust returns them.
public struct CheckRow: Equatable, Sendable {
  /// Which check, spelled as `cox doctor --json` names it.
  public enum Check: String, Sendable {
    case providerKey = "provider_key"
    case git, sandbox
    case shellEnv = "shell_env"
  }

  /// `cox doctor`'s `ok`, `warn` and `fail`.
  public enum Status: Sendable { case passed, warning, failed }

  public var id: Check
  public var status: Status
  public var detail: String

  public init(id: Check, status: Status, detail: String) {
    (self.id, self.status, self.detail) = (id, status, detail)
  }
}

public protocol OnboardingClient: Sendable {
  /// The checklist for a session in `cwd`, whose config names the provider whose key is checked.
  func checklist(cwd: String) async throws -> [CheckRow]
}
