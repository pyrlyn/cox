// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One open session as observable state (DT§4.6): the timeline keyed by
// block id in insertion order (`OrderedDictionary`, research.md 9.5.6), the
// token meter, the composer draft, the terminal tabs and the plugin slots. It
// applies patches and sends intents and nothing else; every decision already
// came from Rust. The rules mirror `cox_app::coalesce::apply`, the reference
// consumer the Rust tests prove.

import CoxClient
import Observation
import OrderedCollections

@Observable
@MainActor
public final class SessionStore {
  public private(set) var blocks: OrderedDictionary<BlockID, Block>
  /// The token meter (DS§7); `nil` until the first `usage` patch.
  public private(set) var usage: UsageView?
  /// A turn runs, as the meter says (`TurnStarted` until `TurnDone`).
  public var isTurnRunning: Bool { usage?.turn.map { !$0.done } ?? false }
  /// What the core reports beside the blocks: the turns queued behind the running one.
  public private(set) var status = Status()
  public var draft = ""
  /// Review's line comments, kept while Review opens other files (T37.28.4).
  public var reviewDraft = ReviewDraft()
  /// The session's terminal panes in tab order (T51.6): the shells run in Rust, and what they
  /// print never reaches `blocks`.
  public internal(set) var terminals: [TerminalTab] = []
  /// The tab the terminal pane shows.
  public var terminalSelection: TerminalTab.ID?
  /// The last tab's id, so a closed tab's id is never handed out again.
  @ObservationIgnored var terminalCount = 0
  /// Each plugin slot's latest state (T52.17, PL§8), in the order the slots first reported.
  public private(set) var pluginSlots: OrderedDictionary<PluginSlotKey, PluginSlot> = [:]
  @ObservationIgnored public let session: any SessionClient

  public init(session: any SessionClient) {
    self.session = session
    blocks = Self.keyed(session.snapshot())
  }

  /// Pulls batches until the session closes or the task is cancelled,
  /// applying each in one main-actor hop (DT§4.5).
  public func run() async {
    while !Task.isCancelled, let batch = await session.nextPatches() {
      apply(batch)
    }
  }

  public func send(_ intent: Intent) async throws -> (any SessionClient)? {
    try await session.send(intent)
  }

  /// Told each batch once the store has applied it, so a view that keeps its own copy of the
  /// timeline — the transcript text (T37.23) — splices the same patches.
  @ObservationIgnored public var didApply: (@MainActor ([TimelinePatch]) -> Void)?

  public func apply(_ patches: [TimelinePatch]) {
    for patch in patches { apply(patch) }
    didApply?(patches)
  }

  private func apply(_ patch: TimelinePatch) {
    switch patch {
    case .reset(let all):
      blocks = Self.keyed(all)
    case .upsert(let block, let after):
      if blocks[block.id] != nil {
        blocks[block.id] = block
        return
      }
      // An unknown `after` appends, as in the Rust consumer.
      let index = after.map { blocks.index(forKey: $0).map { $0 + 1 } ?? blocks.count } ?? 0
      blocks.updateValue(block, forKey: block.id, insertingAt: index)
    case .appendText(let id, let text):
      blocks[id]?.kind.append(text)
    case .docTail(let id, let from, let tail):
      blocks[id]?.kind.replaceDoc(from: Int(from), with: tail)
    case .remove(let id):
      blocks.removeValue(forKey: id)
    case .usage(let view):
      usage = view
    case .status(let new):
      status = new
    case .pluginSlot(let slot):
      // Beside the blocks, as in `cox_app::coalesce`: a reset keeps it, and it replaces only
      // its own slot.
      pluginSlots[slot.key] = slot
    }
  }

  /// Hides the plugin overlay shown, as Esc does; the core answers with the slot's patch.
  public func closePluginOverlay() {
    session.closePluginOverlay()
  }

  /// The column is now `width`×`height` cells; a shown panel or overlay renders again for it.
  public func pluginArea(width: UInt16, height: UInt16) {
    session.pluginArea(width: width, height: height)
  }

  private static func keyed(_ all: [Block]) -> OrderedDictionary<BlockID, Block> {
    OrderedDictionary(all.map { ($0.id, $0) }, uniquingKeysWith: { _, later in later })
  }
}

/// Lines a running tool block keeps (`cox_app::patch::TAIL_LINES`).
let tailLines = 5

extension BlockKind {
  /// A patch that does not fit the block's kind changes nothing.
  mutating func append(_ more: String) {
    switch self {
    case .thinking(let text, let durationMs):
      self = .thinking(text: text + more, durationMs: durationMs)
    case .tool(
      let tool, let summary, let icon, let risk, let state, let tail, let archive, let diff,
      let durationMs, let pluginView):
      self = .tool(
        tool: tool, summary: summary, icon: icon, risk: risk, state: state,
        tail: lastLines(tail + more), archive: archive, diff: diff, durationMs: durationMs,
        pluginView: pluginView)
    default:
      break
    }
  }

  /// A `docTail` carries doc blocks, not source, so the text is left empty rather than a stale
  /// earlier source or a Markdown re-render on every tail; Copy writes the doc through the
  /// core's writer when the text is empty. The reply's closing upsert brings the real source
  /// (`cox_app` `ItemDone`).
  mutating func replaceDoc(from: Int, with tail: [DocBlock]) {
    guard case .assistant(_, var doc, let pluginView) = self, from <= doc.blocks.count else {
      return
    }
    doc.blocks.replaceSubrange(from..., with: tail)
    self = .assistant(text: "", doc: doc, pluginView: pluginView)
  }
}

/// The last `tailLines` lines of `text`, a trailing newline kept
/// (`cox_app::patch::tail`). Bytes, not `Character`s: `\r\n` is one
/// grapheme in Swift but ends a line in Rust.
func lastLines(_ text: String) -> String {
  let bytes = text.utf8
  let newline = UInt8(ascii: "\n")
  var index = bytes.last == newline ? bytes.index(before: bytes.endIndex) : bytes.endIndex
  var seen = 0
  while index > bytes.startIndex {
    index = bytes.index(before: index)
    if bytes[index] == newline {
      seen += 1
      if seen == tailLines { return String(text[bytes.index(after: index)...]) }
    }
  }
  return text
}

extension BlockKind {
  /// The plugin tree a `tool:`/`item:` renderer put on this block; `nil` on every other kind
  /// and until one lands. An upsert replaces it with the block, as a slot patch replaces its
  /// slot.
  public var pluginView: PluginView? {
    switch self {
    case .assistant(_, _, let view), .tool(_, _, _, _, _, _, _, _, _, let view): view
    default: nil
    }
  }
}

extension SessionStore {
  /// The shown plugin views in one slot, in the order the plugins first reported.
  public func pluginViews(_ kind: PluginSlotKind) -> [PluginSlot] {
    pluginSlots.values.filter { $0.slot == kind && $0.visible && $0.view != nil }
  }
}
