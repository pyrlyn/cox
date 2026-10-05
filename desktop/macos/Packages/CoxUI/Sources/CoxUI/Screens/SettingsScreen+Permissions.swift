// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Permissions page's rules and session grants (T37.45.3, A120, mockup 19; DS§6.5 row
// `SettingsScreen`): each allow/ask/deny rule with the layer its list comes from, a row to add
// one, and the "allow for session" grants of the open sessions with a Revoke button.
// Composition only (DS§5): whether a rule is valid, where it is written and what revoking does
// are Rust's; a row reports `.editRule` or `.revokeGrant`, and Rust's refusal comes back as
// `failure`. Apart from `SettingsScreen.swift` so the page's boxes are their own card.

import SwiftUI

extension SettingsScreen {
  /// The Permissions page's own boxes, under its tables.
  public struct Permissions: Equatable, Sendable {
    var rules: [Rule]
    var grants: [Grant]
    /// Why Rust refused the last rule edit or revoke, verbatim.
    var failure: String?

    public init(rules: [Rule], grants: [Grant], failure: String? = nil) {
      (self.rules, self.grants, self.failure) = (rules, grants, failure)
    }
  }

  /// Which list a rule sits in, the badge before it.
  public enum RuleKind: String, CaseIterable, Sendable {
    case allow, ask, deny

    var badge: Badge.Kind {
      switch self {
      case .allow: .env
      case .ask: .warning
      case .deny: .danger
      }
    }
  }

  public struct Rule: Identifiable, Equatable, Sendable {
    public var id: String { "\(kind.rawValue) \(text)" }
    var kind: RuleKind
    var text: String
    var source: SettingSource

    public init(kind: RuleKind, text: String, source: SettingSource) {
      (self.kind, self.text, self.source) = (kind, text, source)
    }
  }

  public struct Grant: Identifiable, Equatable, Sendable {
    /// What `.revokeGrant` names.
    public let id: String
    /// `bash git push`: the tool and the subject prefix it covers.
    var subject: String
    /// The session it was given in.
    var detail: String

    public init(id: String, subject: String, detail: String) {
      (self.id, self.subject, self.detail) = (id, subject, detail)
    }
  }
}

/// The rules in the engine's order with the add row last, then the session's grants.
struct PermissionsBoxes: View {
  let permissions: SettingsScreen.Permissions
  let send: (SettingsScreenIntent) -> Void

  var body: some View {
    SettingsGroupBox("Rules · deny, then allow, then ask") {
      ForEach(permissions.rules) { RuleRow(rule: $0, send: send) }
      AddRuleRow(failure: permissions.failure, send: send)
    }
    Text("A rule applies to sessions opened after the change.")
      .textStyle(.caption)
      .foregroundStyle(Color(.textSecondary))
      .padding(.horizontal, Space.xs)
    SettingsGroupBox("Session grants · this session only") {
      if permissions.grants.isEmpty {
        SettingLabel("No grants", detail: "“Allow for session” in an approval adds one here.")
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(.horizontal, Space.l)
          .padding(.vertical, Space.ml)
      }
      ForEach(permissions.grants) { grant in
        HStack(spacing: Space.l) {
          Text(grant.subject)
            .textStyle(.monoInline)
            .foregroundStyle(Color(.textPrimary))
            .frame(maxWidth: .infinity, alignment: .leading)
          Text(grant.detail)
            .textStyle(.caption)
            .foregroundStyle(Color(.textSecondary))
            .lineLimit(1)
          Button("Revoke") { send(.revokeGrant(id: grant.id)) }
            .buttonStyle(CoxButtonStyle(.danger, size: .small))
        }
        .padding(.horizontal, Space.l)
        .padding(.vertical, Space.ml)
      }
    }
  }
}

/// A rule's kind, its text — edited in place and sent on Return, empty removes it — and the
/// layer badge; `SettingRow` locks a rule a layer above the user file sets.
private struct RuleRow: View {
  let rule: SettingsScreen.Rule
  let send: (SettingsScreenIntent) -> Void
  @State private var draft: String?

  var body: some View {
    SettingRow(source: rule.source) {
      HStack(spacing: Space.ml) {
        KindBadge(kind: rule.kind)
        TextField(rule.text, text: Binding(get: { draft ?? rule.text }, set: { draft = $0 }))
          .textFieldStyle(.plain)
          .textStyle(.monoInline)
          .foregroundStyle(Color(.textPrimary))
          .onSubmit {
            guard let draft, draft != rule.text else { return }
            let new = draft.trimmingCharacters(in: .whitespaces)
            send(.editRule(rule.kind, old: rule.text, new: new.isEmpty ? nil : new))
            self.draft = nil
          }
        if !rule.source.isReadOnly {
          Button {
            send(.editRule(rule.kind, old: rule.text, new: nil))
          } label: {
            Image(systemName: "minus.circle").symbolStyle(.body)
          }
          .buttonStyle(CoxButtonStyle(.plain, size: .small))
          .help("Remove this rule from your config")
          .accessibilityLabel("Remove \(rule.text)")
        }
      }
    }
  }
}

/// A kind picker and a field; Return sends the rule to Rust, which checks it before it saves.
private struct AddRuleRow: View {
  let failure: String?
  let send: (SettingsScreenIntent) -> Void
  @State private var kind = SettingsScreen.RuleKind.allow

  var body: some View {
    VStack(alignment: .leading, spacing: Space.s) {
      HStack(spacing: Space.l) {
        CoxSegmented(
          "Kind", selection: $kind, options: SettingsScreen.RuleKind.allCases,
          title: { Text($0.rawValue) })
        SettingField("", prompt: "Add rule, e.g. Bash(npm test:*)") { text in
          send(.editRule(kind, old: nil, new: text))
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
      }
      if let failure {
        Text(failure)
          .textStyle(.caption)
          .foregroundStyle(Color(.statusDanger))
          .textSelection(.enabled)
      }
    }
    .padding(.horizontal, Space.l)
    .padding(.vertical, Space.ml)
  }
}

/// The kind's badge in a column as wide as the widest kind's, so the rules' text lines up as
/// in the mockup's fixed-width badge column, with no width of its own to keep in step.
private struct KindBadge: View {
  let kind: SettingsScreen.RuleKind

  var body: some View {
    ZStack(alignment: .leading) {
      Badge(SettingsScreen.RuleKind.allow.rawValue).hidden()
      Badge(kind.rawValue, kind: kind.badge)
    }
  }
}

#Preview("permissions") {
  SettingsScreen(state: PreviewState.settingsPermissionRules) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}

#Preview("permissions refused") {
  SettingsScreen(state: PreviewState.settingsPermissionRulesRefused) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}
