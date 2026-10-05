// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The command palette's rows (DT§5.5, T37.44.13), as cox-ffi's `PaletteItem` and `PaletteHit`:
// what the window offers (its actions and the listed sessions) and what the core ranked for a
// query, with the session's `/` commands and `@` files added. Separate from the completion rows
// because a palette row runs something rather than inserting text.

/// What a palette row runs.
public enum PaletteKind: Equatable, Sendable {
  case action, session, command, file
}

/// One row the palette may show; the window supplies actions and sessions.
public struct PaletteItem: Equatable, Sendable {
  public var kind: PaletteKind
  /// An action's name, a session's id, `/name` or `@path`.
  public var id: String
  public var title: String
  /// Keys, or where and when: `⌘⇧R`, `cox · 2 hr. ago`.
  public var detail: String

  public init(kind: PaletteKind, id: String, title: String, detail: String = "") {
    (self.kind, self.id, self.title, self.detail) = (kind, id, title, detail)
  }
}

/// A row that matched the query, and which of its title's characters did.
public struct PaletteHit: Equatable, Sendable {
  public var item: PaletteItem
  public var matched: [Int]

  public init(item: PaletteItem, matched: [Int] = []) {
    (self.item, self.matched) = (item, matched)
  }
}

extension SessionClient {
  /// The rows whose title holds `query` in order, in the order given and with the characters
  /// that did first, then this client's `/`
  /// commands and `@` files for it: enough to drive a view or a test, not the core's ranking,
  /// which the live and remote clients use instead.
  public func palette(_ query: String, items: [PaletteItem], limit: UInt32) -> [PaletteHit] {
    let wanted = query.lowercased()
    var hits: [PaletteHit] = []
    for kind in [PaletteKind.action, .session] {
      let rows = items.filter { $0.kind == kind }.compactMap { item in
        Self.matched(item.title, wanted).map { PaletteHit(item: item, matched: $0) }
      }
      hits += rows.prefix(Int(limit))
    }
    guard !query.isEmpty else { return hits }
    for (sigil, kind) in [("/", PaletteKind.command), ("@", .file)] {
      hits += complete(sigil + query, limit: limit).map {
        // A file's row shows its path; the `@` is what the composer gets.
        let (title, detail) =
          kind == .file ? (String($0.insert.dropFirst()), "") : ($0.insert, $0.detail)
        return PaletteHit(
          item: PaletteItem(kind: kind, id: $0.insert, title: title, detail: detail))
      }
    }
    return hits
  }

  /// The offsets of the title's characters that spell `query` first, or `nil` when it does not.
  private static func matched(_ title: String, _ query: String) -> [Int]? {
    var rest = Substring(query)
    var offsets: [Int] = []
    for (offset, character) in title.lowercased().enumerated() where character == rest.first {
      offsets.append(offset)
      rest = rest.dropFirst()
    }
    return rest.isEmpty ? offsets : nil
  }
}
