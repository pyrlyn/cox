// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Permissions page's values (T37.45.3, A120): the allow/ask/deny rules in effect and the
// grants of the sessions open here, field for field as cox-ffi exports `cox_app::permissions`.
// Separate from `Settings.swift` because a rule is one entry of a list setting, and a grant is
// no setting at all but a session's state its core holds.

/// Which list a rule sits in.
public enum RuleKind: String, CaseIterable, Equatable, Sendable {
  case allow, ask, deny
}

/// One rule in effect.
public struct PermissionRule: Identifiable, Equatable, Sendable {
  public var kind: RuleKind
  /// As written, in the rule grammar.
  public var rule: String
  /// A layer replaces a whole list, so every rule of a kind shares it.
  public var layer: Layer
  /// Whether an edit to the user file would take effect.
  public var editable: Bool

  public var id: String { "\(kind.rawValue) \(rule)" }

  public init(kind: RuleKind, rule: String, layer: Layer, editable: Bool) {
    (self.kind, self.rule, self.layer, self.editable) = (kind, rule, layer, editable)
  }
}

/// One "allow for session" grant of a session open here.
public struct SessionGrant: Identifiable, Equatable, Sendable {
  public var session: String
  public var title: String?
  public var tool: String
  /// The subject prefix the grant covers.
  public var subject: String

  public var id: String { "\(session) \(tool) \(subject)" }

  public init(session: String, title: String? = nil, tool: String, subject: String) {
    (self.session, self.title, self.tool, self.subject) = (session, title, tool, subject)
  }
}

/// Rust refused a rule edit: the rule grammar's message, a list a higher layer sets, or a rule
/// that is no longer there. `description` is Rust's text as it is.
public struct RuleRefused: Error, Equatable, CustomStringConvertible {
  public let message: String
  public var description: String { message }

  public init(_ message: String) { self.message = message }
}
