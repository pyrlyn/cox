// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the user asks a session to do (DT§4.3 Intents), as cox-ffi's
// `Intent`. Separate from the timeline because it only flows the other way:
// the stores send it, nothing decodes it.

public enum Intent: Equatable, Sendable {
  /// `confirmThink`: this one turn goes to the think tier, as `/think` sends it (A103).
  case send(text: String, attachments: [Attachment], confirmThink: Bool = false)
  case approve(call: String, decision: Decision)
  case answer(question: String, text: String?)
  case interrupt
  /// Waits for the running turn to end, keeping the `confirmThink` it was sent with.
  case queue(text: String, attachments: [Attachment], confirmThink: Bool = false)
  case compact(focus: String?)
  case setMode(mode: PermissionMode)
  case switchModel(tier: Tier, model: String?)
  /// The code tier on another provider's `model`, before the first turn (T60.3): the session is
  /// reopened under the same id, and `send` hands back the reopened one to put in its window.
  /// `makeDefault` also writes the pick to the user config.
  case switchProvider(provider: String, model: String, makeDefault: Bool)
  case setEffort(effort: Effort?)
  case rewind(toTurn: UInt32, code: Bool, conversation: Bool)
  case redo
  case revertFile(path: String, toTurn: UInt32)
  /// One hunk of Review's diff back to before `toTurn` (T51.21); `nowDigest` is the diff's
  /// `digest`, so a file changed since Review read it is refused.
  case revertHunk(path: String, toTurn: UInt32, hunk: UInt32, nowDigest: String)
  case fork(turn: UInt32?)
  case handoff(objective: String)
  case background(call: String)
  case shell(command: String, share: Bool)
  case command(line: String)
  /// The person's title for the session (A113), as `/rename` sets it.
  case rename(title: String)
}

/// The core refused a provider switch because the session already ran a turn (`AppError::
/// ProviderLocked`, T60.3); `message` is Rust's text, and the window offers a new session.
public struct ProviderLocked: Error, Equatable, CustomStringConvertible {
  public let message: String
  public var description: String { message }

  public init(_ message: String) { self.message = message }
}

public struct Attachment: Equatable, Sendable {
  public var name: String
  public var mediaType: String
  public var dataB64: String

  public init(name: String, mediaType: String, dataB64: String) {
    (self.name, self.mediaType, self.dataB64) = (name, mediaType, dataB64)
  }
}

public enum PermissionMode: String, Equatable, Sendable, Decodable {
  case `default`, plan, auto, bypass
}

public enum Effort: String, Equatable, Sendable, Decodable { case low, medium, high, xhigh }
