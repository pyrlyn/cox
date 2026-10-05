// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer's rules as cox-ffi returns them (T58.4.16, T58.4.17): the token at the caret
// that asks for rows, a picked row spliced into the draft, the picked `@` files still in it, and
// what a draft becomes when sent. `cox_app::complete` and `cox_app::intent` decide; the stores
// only apply the answer. Separate from `CoreClient.swift`, which is the seam, not its values.

import Foundation

/// The word the caret ends when it asks for rows; offsets are UTF-16 units into the draft.
public struct TypedToken: Equatable, Sendable {
  public var start: Int
  public var end: Int
  /// `@que…` or `/que…`, what `SessionClient.complete` takes.
  public var text: String

  public init(start: Int, end: Int, text: String) {
    (self.start, self.end, self.text) = (start, end, text)
  }
}

/// A draft after a pick: its text and the caret, in UTF-16 units.
public struct Splice: Equatable, Sendable {
  public var text: String
  public var caret: Int

  public init(text: String, caret: Int) { (self.text, self.caret) = (text, caret) }
}

/// When a turn goes while another runs, spelled as Rust stores `[desktop.review] send` (A108).
public enum SendWhen: String, Equatable, Sendable, Decodable {
  /// Behind the running turn, as the composer queues a prompt; at once when no turn runs.
  case queue
  /// At once, even while a turn runs.
  case now
}

/// Which intent a draft is sent as.
public enum DraftKind: Equatable, Sendable {
  case shell, command, turn
}

/// What a draft becomes (`cox_app::intent::DraftIntent`).
public struct DraftIntent: Equatable, Sendable {
  public var kind: DraftKind
  /// A turn held behind the running one (`Intent.queue`).
  public var queued: Bool
  public var canSend: Bool
  /// The attachments and the think toggle stay: a shell or command line cannot carry them.
  public var keepsAttachments: Bool
  /// The draft is a lone `!`: shell mode instead of text.
  public var entersShell: Bool

  public init(
    kind: DraftKind, queued: Bool, canSend: Bool, keepsAttachments: Bool, entersShell: Bool
  ) {
    (self.kind, self.queued, self.canSend) = (kind, queued, canSend)
    (self.keepsAttachments, self.entersShell) = (keepsAttachments, entersShell)
  }
}

extension FixtureSession {
  /// The rules as the core has them, over Swift strings: enough to drive a view or a test; the
  /// live and remote clients ask Rust instead.
  public func typedToken(_ text: String, caret: Int, selection: Bool, shell: Bool) -> TypedToken? {
    guard !shell, !selection, let end = Self.index(text, caret),
      end == text.endIndex || text[end].isWhitespace,
      let last = text[..<end].last, !last.isWhitespace
    else { return nil }
    let start = text[..<end].lastIndex(where: \.isWhitespace).map { text.index(after: $0) }
    let token = text[(start ?? text.startIndex)..<end]
    guard token.hasPrefix("@") || (token.hasPrefix("/") && start == nil) else { return nil }
    let offset = text.utf16.distance(from: text.startIndex, to: token.startIndex)
    return TypedToken(start: offset, end: caret, text: String(token))
  }

  public func pick(_ text: String, token: TypedToken, insert: String) -> Splice? {
    guard let start = Self.index(text, token.start), let end = Self.index(text, token.end),
      start <= end
    else { return nil }
    var head = String(text[..<start])
    if start == end, !head.isEmpty, !head.hasSuffix(" ") { head += " " }
    head += insert + " "
    let rest = text[end...]
    let tail = rest.first == " " ? rest.dropFirst() : rest
    return Splice(text: head + tail, caret: head.utf16.count)
  }

  public func mentions(_ text: String, picked: [String]) -> [String] {
    var kept: [String] = []
    for insert in picked where insert.hasPrefix("@") && text.contains(insert) {
      if !kept.contains(insert) { kept.append(insert) }
    }
    return kept
  }

  public func draftIntent(
    _ text: String, shell: Bool, attachments: Int, running: Bool, when: SendWhen
  ) -> DraftIntent {
    let kind: DraftKind = shell ? .shell : text.hasPrefix("/") ? .command : .turn
    return DraftIntent(
      kind: kind, queued: kind == .turn && running && when == .queue,
      canSend: !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        || (!shell && attachments > 0),
      keepsAttachments: kind != .turn, entersShell: !shell && text == "!")
  }

  /// `offset` UTF-16 units into `text`, when it falls between characters.
  private static func index(_ text: String, _ offset: Int) -> String.Index? {
    guard offset >= 0, offset <= text.utf16.count else { return nil }
    return String.Index(text.utf16.index(text.utf16.startIndex, offsetBy: offset), within: text)
  }
}
