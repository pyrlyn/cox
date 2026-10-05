// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The first-run checklist over cox-ffi (DT§5.8, T37.31): `App.checklist` and its rows as
// CoxClient's `CheckRow`. Separate from `LiveCoreClient.swift` like the other conversions, so
// that file stays the list of calls into Rust.

import CoxClient
import CoxFFIBindings

extension LiveCoreClient: OnboardingClient {
  public func checklist(cwd: String) async throws -> [CoxClient.CheckRow] {
    try await app.checklist(cwd: cwd).map { CoxClient.CheckRow($0) }
  }
}

extension CoxClient.CheckRow {
  init(_ row: CoxFFIBindings.CheckRow) {
    let id: Check =
      switch row.id {
      case .providerKey: .providerKey
      case .git: .git
      case .sandbox: .sandbox
      case .shellEnv: .shellEnv
      }
    let status: Status =
      switch row.status {
      case .ok: .passed
      case .warn: .warning
      case .fail: .failed
      }
    self.init(id: id, status: status, detail: row.detail)
  }
}
