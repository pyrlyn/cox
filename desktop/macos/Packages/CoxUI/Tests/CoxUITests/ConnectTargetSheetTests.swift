// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T68.5's CoxUI check (DS§6.4, DT§5.10): the Connect to Target sheet listing a kind's targets with
// one picked, after a failed connect, and on `hello` with and without a sandbox, each in
// light/dark × Solid/Frosted. The reference images come from the `desktop-macos-snapshots` CI
// artifact, never from a local run: this Mac's macOS is newer than the runner's.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ConnectTargetSheetSnapshotTests {
  @Test(arguments: Variant.all) func listed(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane {
        ConnectTargetSheet(
          state: PreviewState.connectTargetListed, kind: "docker", picked: "docker:web"
        ) { _ in }
      }, variant, named: "listed.\(variant.name)")
  }

  @Test(arguments: Variant.all) func failed(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane {
        ConnectTargetSheet(state: PreviewState.connectTargetFailed, kind: "ssh", name: "devbox") {
          _ in
        }
      }, variant, named: "failed.\(variant.name)")
  }

  @Test(arguments: Variant.all) func hello(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { ConnectTargetSheet(state: PreviewState.connectTargetHello) { _ in } }, variant,
      named: "hello.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { ConnectTargetSheet(state: PreviewState.connectTargetNoSandbox) { _ in } },
      variant, named: "no-sandbox.\(variant.name)")
  }
}
