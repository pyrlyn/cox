// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The 2 000-block transcript both engines draw: prose, code, diffs and tool
// cards in a fixed rotation, deterministic so every run measures the same text.
// Each block carries its Markdown source, which is what a copy must return.

import Foundation

public enum BlockKind: String, Sendable {
  case prose, code, diff, tool
}

public struct Block: Identifiable, Sendable {
  public let id: Int
  public let kind: BlockKind
  /// The block as Markdown: what "Copy" puts on the pasteboard for a whole block.
  public let markdown: String
  /// The text a reader sees (prose keeps inline Markdown; code and diffs are the fence body;
  /// a tool card is its one-line summary).
  public let body: String
  /// Tool cards only: the tool name shown in the card header.
  public let tool: String?
}

public enum Fixture {
  /// prose, code, prose, diff, tool — prose is 40 %, the others 20 % each.
  static let rotation: [BlockKind] = [.prose, .code, .prose, .diff, .tool]

  public static func blocks(_ count: Int = 2_000) -> [Block] {
    (0..<count).map { block($0, kind: rotation[$0 % rotation.count]) }
  }

  static func block(_ index: Int, kind: BlockKind) -> Block {
    switch kind {
    case .prose:
      let sentences = 2 + index % 5
      let body = (0..<sentences).map { sentence in
        "Block \(index) sentence \(sentence) explains **why** the `turn_\(index)` step "
          + "ran before the compaction and what the model saw next."
      }.joined(separator: " ")
      return Block(id: index, kind: kind, markdown: body, body: body, tool: nil)
    case .code:
      let lines = 3 + index % 9
      let body = (0..<lines).map { line in
        "let value\(line) = compute(block: \(index), line: \(line)) // step \(line)"
      }.joined(separator: "\n")
      return Block(
        id: index, kind: kind, markdown: "```swift\n\(body)\n```", body: body, tool: nil)
    case .diff:
      let body = [
        "@@ -\(index),3 +\(index),3 @@",
        " fn turn_\(index)() {",
        "-    let budget = \(index);",
        "+    let budget = \(index + 1);",
        " }",
      ].joined(separator: "\n")
      return Block(
        id: index, kind: kind, markdown: "```diff\n\(body)\n```", body: body, tool: nil)
    case .tool:
      let tool = ["bash", "read", "edit", "grep"][index % 4]
      let body = "cargo nextest run -p cox-core -- turn_\(index) · exit 0 · 1.\(index % 10) s"
      return Block(
        id: index, kind: kind, markdown: "> **\(tool)** `\(body)`", body: body, tool: tool)
    }
  }

  /// The whole transcript as one Markdown document, blocks separated by a blank line.
  public static func markdown(_ blocks: [Block]) -> String {
    blocks.map(\.markdown).joined(separator: "\n\n")
  }
}
