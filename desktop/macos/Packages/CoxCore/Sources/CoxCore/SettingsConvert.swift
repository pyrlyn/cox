// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings records from cox-ffi into CoxClient's (DT§5.7), apart from
// `Convert.swift`'s timeline so each file stays one concern. Field for
// field; nothing is decided here.

import CoxClient
import CoxFFIBindings

extension CoxClient.SettingsView {
  init(_ view: CoxFFIBindings.SettingsView) {
    self.init(
      settings: view.settings.map { CoxClient.Setting($0) }, userFile: view.userFile,
      projectFile: view.projectFile, mcp: view.mcp.map { CoxClient.McpServer($0) },
      dropped: view.dropped.map { CoxClient.Dropped($0) },
      rules: view.rules.map { CoxClient.PermissionRule($0) },
      grants: view.grants.map { CoxClient.SessionGrant($0) }, providers: view.providers)
  }
}

extension CoxClient.PermissionRule {
  init(_ rule: CoxFFIBindings.PermissionRule) {
    self.init(
      kind: .init(rule.kind), rule: rule.rule, layer: .init(rule.layer), editable: rule.editable)
  }
}

extension CoxClient.RuleKind {
  init(_ kind: CoxFFIBindings.RuleKind) {
    switch kind {
    case .allow: self = .allow
    case .ask: self = .ask
    case .deny: self = .deny
    }
  }
}

extension CoxFFIBindings.RuleKind {
  init(_ kind: CoxClient.RuleKind) {
    switch kind {
    case .allow: self = .allow
    case .ask: self = .ask
    case .deny: self = .deny
    }
  }
}

extension CoxClient.SessionGrant {
  init(_ grant: CoxFFIBindings.SessionGrant) {
    self.init(session: grant.session, title: grant.title, tool: grant.tool, subject: grant.subject)
  }
}

extension CoxFFIBindings.SessionGrant {
  /// Back to Rust for a revoke, which matches on the session, tool and subject.
  init(_ grant: CoxClient.SessionGrant) {
    self.init(session: grant.session, title: grant.title, tool: grant.tool, subject: grant.subject)
  }
}

extension CoxClient.Dropped {
  init(_ dropped: CoxFFIBindings.Dropped) {
    self.init(
      key: dropped.key, value: dropped.value, kept: dropped.kept, reason: dropped.reason,
      group: .init(dropped.group), change: dropped.change)
  }
}

extension CoxClient.McpServer {
  init(_ server: CoxFFIBindings.McpServer) {
    self.init(
      name: server.name, source: server.source, login: .init(server.login),
      detail: server.detail, action: server.action.map { .init($0) }, status: .init(server.status),
      log: server.log)
  }
}

extension CoxClient.McpLoginAction {
  init(_ action: CoxFFIBindings.LoginAction) {
    switch action {
    case .logIn: self = .logIn
    case .logOut: self = .logOut
    }
  }
}

extension CoxClient.McpStatus {
  init(_ status: CoxFFIBindings.McpStatus) {
    switch status {
    case .connected: self = .connected
    case .needsLogin: self = .needsLogin
    case .failed: self = .failed
    case .disabled: self = .disabled
    case .unknown: self = .unknown
    }
  }
}

extension CoxClient.McpLogin {
  init(_ login: CoxFFIBindings.McpLogin) {
    switch login {
    case .stdio: self = .stdio
    case .loggedOut: self = .loggedOut
    case .loggedIn(let expires): self = .loggedIn(expires: expires)
    case .expired: self = .expired
    case .unreadable(let error): self = .unreadable(error: error)
    }
  }
}

extension CoxClient.Setting {
  init(_ setting: CoxFFIBindings.Setting) {
    self.init(
      key: setting.key, value: setting.value, layer: .init(setting.layer),
      editable: setting.editable, kind: .init(setting.kind), description: setting.description,
      group: .init(setting.group), title: setting.title, table: setting.table,
      provider: setting.provider, detail: setting.detail, control: .init(setting.control))
  }
}

extension CoxClient.SettingsGroup {
  init(_ group: CoxFFIBindings.SettingsGroup) {
    switch group {
    case .general: self = .general
    case .models: self = .models
    case .permissions: self = .permissions
    case .sandbox: self = .sandbox
    case .budget: self = .budget
    case .mcp: self = .mcp
    case .plugins: self = .plugins
    case .appearance: self = .appearance
    case .advanced: self = .advanced
    }
  }
}

extension CoxClient.Setting.Control {
  /// Rust sends a slider only with `min` below `max`, so the range is well formed.
  init(_ control: CoxFFIBindings.SettingControl) {
    switch control {
    case .toggle(let isOn): self = .toggle(isOn)
    case .slider(let value, let min, let max, let text):
      self = .slider(value, range: min...max, text: text)
    case .choice(let value, let options): self = .choice(value, options: options)
    case .menu(let value, let options):
      self = .menu(value, options: options.map { .init(value: $0.value, title: $0.title) })
    case .field(let text): self = .field(text)
    case .json(let text): self = .json(text)
    }
  }
}

extension CoxFFIBindings.SettingInput {
  init(_ value: CoxClient.SettingValue) {
    switch value {
    case .bool(let value): self = .bool(value: value)
    case .integer(let value): self = .integer(value: value)
    case .number(let value): self = .number(value: value)
    case .text(let value): self = .text(value: value)
    case .list(let values): self = .list(values: values)
    }
  }
}

extension CoxClient.KeyError {
  init(_ error: CoxFFIBindings.KeyError) {
    switch error {
    case .Empty: self = .empty
    case .UnknownProvider(let provider): self = .unknownProvider(provider)
    }
  }
}

extension CoxClient.Layer {
  init(_ layer: CoxFFIBindings.Layer) {
    switch layer {
    case .default: self = .default
    case .user: self = .user
    case .project: self = .project
    case .claudeSettings: self = .claudeSettings
    case .env: self = .env
    case .flag: self = .flag
    }
  }
}

extension CoxClient.SettingKind {
  init(_ kind: CoxFFIBindings.SettingKind) {
    switch kind {
    case .toggle: self = .toggle
    case .integer(let min, let max): self = .integer(min: min, max: max)
    case .number(let min, let max): self = .number(min: min, max: max)
    case .text: self = .text
    case .choice(let options): self = .choice(options: options)
    case .list: self = .list
    case .other: self = .other
    }
  }
}
