// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// How the timeline values read the serde JSON a recorded fixture holds
// (DT§8): internally tagged enums (`#[serde(tag = "type")]`), an
// externally tagged `TextKind`, spans with their defaults omitted. Separate
// from `Timeline.swift`, which declares the values, because only fixtures
// decode; live values arrive already typed through CoxCore.

import Foundation

/// Any key, so one container reads every variant of an internally tagged
/// enum (`#[serde(tag = "type")]`).
struct AnyKey: CodingKey {
  let stringValue: String
  var intValue: Int? { nil }
  init(_ name: String) { stringValue = name }
  init(stringValue: String) { self.stringValue = stringValue }
  init?(intValue: Int) { nil }
}

extension KeyedDecodingContainer where K == AnyKey {
  func callAsFunction<T: Decodable>(_ key: String) throws -> T {
    try decode(T.self, forKey: AnyKey(key))
  }

  func optional<T: Decodable>(_ key: String) throws -> T? {
    try decodeIfPresent(T.self, forKey: AnyKey(key))
  }

  func unknown(_ tag: String) -> DecodingError {
    .dataCorrupted(.init(codingPath: codingPath, debugDescription: "unknown variant \(tag)"))
  }
}

extension Decoder {
  func fields() throws -> KeyedDecodingContainer<AnyKey> { try container(keyedBy: AnyKey.self) }
}

extension BlockKind: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("type")
    switch tag {
    case "user": self = try .user(text: keys("text"), attachments: keys("attachments"))
    case "assistant":
      self = try .assistant(
        text: keys("text"), doc: keys("doc"), pluginView: keys.optional("plugin_view"))
    case "thinking":
      self = try .thinking(text: keys("text"), durationMs: keys.optional("duration_ms"))
    case "tool":
      self = try .tool(
        tool: keys("tool"), summary: keys("summary"), icon: keys("icon"), risk: keys("risk"),
        state: keys("state"), tail: keys("tail"), archive: keys.optional("archive"),
        diff: keys.optional("diff"), durationMs: keys("duration_ms"),
        pluginView: keys.optional("plugin_view"))
    case "tool_group":
      self = try .toolGroup(
        summary: keys("summary"), children: keys("children"), state: keys("state"))
    case "approval":
      let input: JSONValue = try keys("input")
      self = try .approval(
        call: keys("call"), tool: keys("tool"), summary: keys("summary"), input: input.text(),
        grants: keys("grants"), why: keys("why"), source: keys.optional("source"),
        decision: keys.optional("decision"), by: keys.optional("by"))
    case "question":
      self = try .question(
        call: keys("call"), question: keys("question"), options: keys("options"),
        answer: keys.optional("answer"))
    case "task":
      self = try .task(
        task: keys("task"), label: keys("label"), tier: keys("tier"), done: keys("done"),
        costUsd: keys("cost_usd"), exitCode: keys.optional("exit_code"), state: keys("state"),
        kind: keys("kind"))
    case "compaction":
      self = try .compaction(
        beforeTokens: keys("before_tokens"), afterTokens: keys("after_tokens"),
        reason: keys("reason"),
        summary: keys.optional("summary"))
    case "checkpoint": self = try .checkpoint(files: keys("files"))
    case "notice": self = try .notice(level: keys("level"), text: keys("text"))
    case "error": self = try .error(text: keys("text"), fatal: keys("fatal"))
    case "turn_meta":
      self = try .turnMeta(
        model: keys("model"), tier: keys("tier"), usage: keys.optional("usage"),
        stop: keys.optional("stop"))
    default: throw keys.unknown(tag)
    }
  }
}

extension TimelinePatch: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("op")
    switch tag {
    case "reset": self = try .reset(blocks: keys("blocks"))
    case "upsert": self = try .upsert(block: keys("block"), after: keys.optional("after"))
    case "append_text": self = try .appendText(id: keys("id"), text: keys("text"))
    case "doc_tail": self = try .docTail(id: keys("id"), from: keys("from"), blocks: keys("blocks"))
    case "remove": self = try .remove(id: keys("id"))
    case "usage": self = try .usage(usage: keys("usage"))
    case "status": self = try .status(status: keys("status"))
    default: throw keys.unknown(tag)
    }
  }
}

extension StopReason: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("type")
    switch tag {
    case "end_turn": self = .endTurn
    case "max_turns": self = .maxTurns
    case "interrupted": self = .interrupted
    case "budget": self = .budget
    case "error": self = .error
    case "refusal": self = try .refusal(detail: keys("detail"))
    default: throw keys.unknown(tag)
    }
  }
}

extension Decision: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("type")
    switch tag {
    case "allow": self = .allow
    case "allow_for_session": self = .allowForSession
    case "deny": self = try .deny(reason: keys("reason"))
    case "edit":
      let input: JSONValue = try keys("input")
      self = .edit(input: try input.text())
    default: throw keys.unknown(tag)
    }
  }
}

extension Why: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("type")
    switch tag {
    case "rule_ask": self = try .ruleAsk(rule: keys("rule"))
    case "risk": self = try .risk(risk: keys("risk"))
    case "sandbox_denied": self = try .sandboxDenied(detail: keys("detail"))
    case "policy": self = try .policy(policy: keys("policy"))
    default: throw keys.unknown(tag)
    }
  }
}

extension DocBlock: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("type")
    switch tag {
    case "text": self = try .text(kind: keys("kind"), lines: keys("lines"))
    case "code": self = try .code(lang: keys("lang"), lines: keys("lines"))
    case "table": self = try .table(rows: keys("rows"))
    case "rule": self = .rule
    default: throw keys.unknown(tag)
    }
  }
}

extension TextKind: Decodable {
  /// Externally tagged: `"paragraph"`, or `{"heading": 2}`.
  public init(from decoder: any Decoder) throws {
    if let unit = try? decoder.singleValueContainer().decode(String.self) {
      switch unit {
      case "paragraph": self = .paragraph
      case "list": self = .list
      case "quote": self = .quote
      default: throw try decoder.fields().unknown(unit)
      }
      return
    }
    self = try .heading(decoder.fields()("heading"))
  }
}

extension TextLine: Decodable {
  /// serde omits `quote`, `depth` and `marker` at their defaults.
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    try self.init(
      keys("spans"), quote: keys.optional("quote") ?? 0, depth: keys.optional("depth") ?? 0,
      marker: keys.optional("marker") ?? "")
  }
}

extension Span: Decodable {
  /// serde omits every field left at its default; `rgb` and `light` are `[r, g, b]`.
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let colour = { (key: String) in
      try (keys.optional(key) as [UInt8]?).map { $0.reduce(0) { $0 << 8 | UInt32($1) } }
    }
    text = try keys.optional("text") ?? ""
    token = try keys.optional("token") ?? .text
    (rgb, light) = (try colour("rgb"), try colour("light"))
    bold = try keys.optional("bold") ?? false
    italic = try keys.optional("italic") ?? false
    strike = try keys.optional("strike") ?? false
    underline = try keys.optional("underline") ?? false
    link = try keys.optional("link")
  }
}

extension PluginView: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    let tag: String = try keys("type")
    switch tag {
    case "text": self = try .text(lines: keys("lines"))
    case "list":
      self = try .list(items: keys("items"), selected: keys.optional("selected"))
    case "table": self = try .table(header: keys("header"), rows: keys("rows"))
    case "key_value": self = try .keyValue(rows: keys("rows"))
    case "gauge": self = try .gauge(ratio: keys("ratio"), label: keys("label"))
    case "stack":
      self = try .stack(vertical: keys("vertical"), children: keys("children"))
    case "block":
      self = try .block(title: keys.optional("title"), child: keys("child"))
    default: throw keys.unknown(tag)
    }
  }
}

extension PluginRun: Decodable {
  /// serde names the role `style` and omits `bold`/`italic` at their defaults.
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    try self.init(
      keys.optional("text") ?? "", token: keys.optional("style") ?? .text,
      bold: keys.optional("bold") ?? false, italic: keys.optional("italic") ?? false)
  }
}

extension PluginRow: Decodable {
  public init(from decoder: any Decoder) throws {
    let keys = try decoder.fields()
    try self.init(key: keys("key"), value: keys("value"))
  }
}

/// Any JSON, read only to turn an edited tool input back into JSON text.
enum JSONValue: Codable {
  case null
  case bool(Bool)
  case number(Double)
  case string(String)
  case array([JSONValue])
  case object([String: JSONValue])

  init(from decoder: any Decoder) throws {
    let container = try decoder.singleValueContainer()
    if container.decodeNil() {
      self = .null
    } else if let bool = try? container.decode(Bool.self) {
      self = .bool(bool)
    } else if let number = try? container.decode(Double.self) {
      self = .number(number)
    } else if let string = try? container.decode(String.self) {
      self = .string(string)
    } else if let array = try? container.decode([JSONValue].self) {
      self = .array(array)
    } else {
      self = .object(try container.decode([String: JSONValue].self))
    }
  }

  func encode(to encoder: any Encoder) throws {
    var container = encoder.singleValueContainer()
    switch self {
    case .null: try container.encodeNil()
    case .bool(let bool): try container.encode(bool)
    case .number(let number): try container.encode(number)
    case .string(let string): try container.encode(string)
    case .array(let array): try container.encode(array)
    case .object(let object): try container.encode(object)
    }
  }

  func text() throws -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = .sortedKeys
    return String(bytes: try encoder.encode(self), encoding: .utf8) ?? "null"
  }
}
