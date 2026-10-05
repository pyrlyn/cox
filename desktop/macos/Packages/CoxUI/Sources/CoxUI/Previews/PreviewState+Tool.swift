// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the tool card's molecules (T37.21.1, T37.21.4): tool headers and
// terminal tails, as mockup screen 28's transcript shows them. Separate from `PreviewState.swift`
// so molecules built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// A finished edit with its line counts, the mockup's first tool card.
  static let toolEdited = ToolHeader.Item(
    tile: .edit, symbol: symbol(.edit), verb: "Edited",
    subject: "crates/cox-provider-http/src/retry.rs", change: .init(added: 18, removed: 4),
    state: .succeeded, duration: "0.1 s")

  /// A command still running, its subject monospaced.
  static let toolRunning = ToolHeader.Item(
    tile: .shell, symbol: symbol(.shell), verb: "Running",
    subject: "cargo nextest run -p cox-provider-http", subjectIsCode: true, state: .running,
    duration: "12 s")

  /// A search over several files, with a secondary detail.
  static let toolExplored = ToolHeader.Item(
    tile: .search, symbol: symbol(.search), verb: "Explored", subject: "6 files",
    detail: "· retry.rs, http.rs, sse.rs, lib.rs +2", state: .succeeded, duration: "1.1 s")

  /// A risky call that failed.
  static let toolFailed = ToolHeader.Item(
    tile: .shell, symbol: symbol(.shell), verb: "Ran", subject: "git push origin wt/retry-jitter",
    subjectIsCode: true, risk: .init(text: risk, level: .high), state: .failed,
    duration: "2.4 s")
}

/// A tool header across the reading column.
struct ToolHeaderSample: View {
  let item: ToolHeader.Item
  let isExpanded: Bool?

  init(_ item: ToolHeader.Item, isExpanded: Bool? = nil) {
    self.item = item
    self.isExpanded = isExpanded
  }

  var body: some View {
    ToolHeader(item, isExpanded: isExpanded).frame(width: Size.readingWidth)
  }
}

extension PreviewState {
  /// The mockup's running `cargo nextest` tail.
  static let tail = [
    "   Compiling cox-provider-http v0.9.0 (crates/cox-provider-http)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.84s",
    "    Starting 38 tests across 4 binaries",
    "        PASS [   0.004s] cox-provider-http retry::tests::delay_never_exceeds_cap",
    "        PASS [   0.006s] cox-provider-http retry::tests::jitter_is_uniform_over_seeded_rng",
  ]

  static let tailSucceeded = TerminalTail.Exit.succeeded("exit 0 · 12.4 s")
  static let tailFailed = TerminalTail.Exit.failed("exit 101 · 12.4 s")
}

/// The mockup's tail under a tool row in the reading column.
struct TerminalTailSample: View {
  let exit: TerminalTail.Exit

  var body: some View {
    TerminalTail(PreviewState.tail, exit: exit).frame(width: Size.readingWidth)
  }
}
