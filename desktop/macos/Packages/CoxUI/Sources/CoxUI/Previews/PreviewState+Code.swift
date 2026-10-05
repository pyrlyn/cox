// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the code molecules (T37.21.2, T37.21.3): the retry-jitter diff from
// mockup screen 28 and the test the turn adds for its cap, as highlighted runs the core would send. Separate from
// `PreviewState.swift` so molecules built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  static let hunkHeader = "@@ -41,12 +41,26 @@ impl Backoff"

  /// The mockup's `retryDiff`: context, one removed line, four added lines, context.
  static let hunk: [DiffLineView.Line] = [
    .init(
      kind: .context, number: "41",
      runs: [
        .init("pub fn", .keyword), .init(" "), .init("delay", .function), .init("(&"),
        .init("self", .keyword), .init(", attempt: "), .init("u32", .type), .init(") -> "),
        .init("Duration", .type), .init(" {"),
      ]),
    .init(
      kind: .removed, number: "42",
      runs: [
        .init("    "), .init("self", .keyword), .init(".base * "), .init("2", .number),
        .init("u32", .keyword), .init(".pow(attempt)"),
      ]),
    .init(
      kind: .added, number: "42",
      runs: [
        .init("    "), .init("let", .keyword), .init(" ceiling = "), .init("self", .keyword),
        .init(".base.saturating_mul("), .init("1", .number), .init(" << attempt.min("),
        .init("16", .number), .init("));"),
      ]),
    .init(
      kind: .added, number: "43",
      runs: [
        .init("    "), .init("let", .keyword), .init(" capped = ceiling.min("),
        .init("self", .keyword), .init(".cap);"),
      ]),
    .init(
      kind: .added, number: "44",
      runs: [
        .init("    "),
        .init("// full jitter: uniform in [0, capped], so retries spread out", .comment),
      ]),
    .init(
      kind: .added, number: "45",
      runs: [
        .init("    capped.mul_f64("), .init("self", .keyword), .init(".rng.lock().gen::<"),
        .init("f64", .type), .init(">())"),
      ]),
    .init(kind: .context, number: "46", runs: [.init("}")]),
  ]

  static let codeLanguage = "rust"

  /// The cap test the turn writes, with a line longer than the reading column.
  static let code: [[CodeRun]] = [
    [.init("#[test]", .function)],
    [
      .init("fn", .keyword), .init(" "), .init("delay_never_exceeds_cap", .function), .init("() {"),
    ],
    [
      .init("    "), .init("let", .keyword), .init(" backoff = "), .init("Backoff", .type),
      .init("::new("), .init("Duration", .type), .init("::from_millis("), .init("100", .number),
      .init("), "), .init("Duration", .type), .init("::from_secs("), .init("30", .number),
      .init("), "), .init("StdRng", .type), .init("::seed_from_u64("), .init("7", .number),
      .init("));"),
    ],
    [
      .init("    "), .init("assert!", .function), .init("(backoff.delay("), .init("40", .number),
      .init(") <= "), .init("Duration", .type), .init("::from_secs("), .init("30", .number),
      .init(")); "), .init("// capped", .comment),
    ],
    [.init("}")],
  ]
}

/// The mockup's first line of each kind across the reading column.
struct DiffLineSample: View {
  let kind: DiffLineView.Kind

  var body: some View {
    let line = PreviewState.hunk.first { $0.kind == kind } ?? PreviewState.hunk[0]
    DiffLineView(line).frame(width: Size.readingWidth)
  }
}

/// The mockup's hunk across the reading column; with `revert`, Review's header (T51.21), its
/// "Revert hunk" revealed when `isHovered`.
struct DiffHunkSample: View {
  var revert = false
  var isHovered = false

  var body: some View {
    DiffHunkView(
      header: PreviewState.hunkHeader, lines: PreviewState.hunk, revert: revert ? {} : nil,
      isHovered: isHovered
    )
    .frame(width: Size.readingWidth)
  }
}

/// The cap test in a block across the reading column, with or without its language.
struct CodeBlockSample: View {
  let language: String?

  var body: some View {
    CodeBlockView(language: language, lines: PreviewState.code) {}
      .frame(width: Size.readingWidth)
  }
}
