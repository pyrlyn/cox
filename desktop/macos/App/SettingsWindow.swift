// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings window (DT§5.7, T37.30.1): `SettingsStore`'s pages, tables and fields copied into
// CoxUI's `SettingsScreenState` case for case, and the screen's intents sent back to the store.
// A slider's steps are coalesced into one write per rest; until Rust answers, the slider shows
// the value it was dragged to. Wiring only: what each field shows was decided in CoxModel. The
// window wears the session window's chrome and appearance, as the mockup's Settings does.

import CoxClient
import CoxModel
import CoxUI
import SwiftUI

struct SettingsWindow: View {
  let model: AppModel
  @State private var page = SettingsPage.general
  @State private var sliderWrites = Coalescer()
  /// Slider values sent but not yet stored, by key.
  @State private var dragged: [String: Double] = [:]
  /// Why the last key could not be stored; shown until dismissed.
  @State private var refused: String?
  @Environment(\.coxAppearance) private var base

  var body: some View {
    Group {
      if let settings = model.settings {
        SettingsScreen(state: state(settings), recorder: Hotkeys.recorder) {
          handle($0, settings)
        }
        .task {
          sliderWrites.onIdle = { dragged = [:] }
          await settings.load()
        }
      } else if case .failure(let error) = model.launch.live {
        Text(String(describing: error)).textSelection(.enabled).padding(Space.xxl)
      }
    }
    .frame(minWidth: Size.windowMinWidth, minHeight: Size.windowMinHeight)
    .environment(\.coxAppearance, appearance.applied(to: base))
    .behindWindowBlur(
      appearance.blurFraction, tint: appearance.tint,
      in: RoundedRectangle(cornerRadius: Radius.window, style: .continuous)
    )
    .seeThroughWindow()
    .alert(refused ?? "", isPresented: isRefused) {}
  }

  /// `[desktop.appearance]` as the session window draws it; the defaults until settings load.
  private var appearance: AppearancePopover.State {
    model.settings.map { AppearancePopover.State($0) } ?? AppearancePopover.State()
  }

  private var isRefused: Binding<Bool> {
    Binding(get: { refused != nil }, set: { if !$0 { refused = nil } })
  }

  private func state(_ settings: SettingsStore) -> SettingsScreenState {
    let sections = settings.sections
    let pages = sections.compactMap { SettingsPage(rawValue: $0.group.rawValue) }
    // A search that hides the chosen page shows the first page it keeps.
    let shown = pages.contains(page) ? page : pages.first ?? page
    let section = sections.first { $0.group.rawValue == shown.rawValue }
    let tables = section.map { settings.tables(in: $0) } ?? []
    // A search that keeps no page shows no page's logins or dropped values either.
    let matchesNothing = section == nil && !settings.filter.isEmpty
    let group = matchesNothing ? nil : SettingsGroup(rawValue: shown.rawValue)
    return SettingsScreenState(
      pages: pages,
      selection: shown,
      tables: tables.map { table in
        SettingsScreen.Table(
          id: table.name, fields: table.fields.map(field),
          key: table.provider.map { .init(provider: $0, isStored: settings.hasKey(for: $0)) })
      },
      userFile: settings.view?.userFile ?? "", projectFile: settings.view?.projectFile,
      logins: shown == .mcp && !matchesNothing
        ? settings.logins.map {
          .init(
            id: $0.server, detail: $0.detail, action: action($0), status: Self.status($0.status),
            log: $0.log)
        } : [],
      dropped: group.map { group in
        settings.dropped(in: group).map { .init(id: $0.key, reason: $0.reason, change: $0.change) }
      } ?? [],
      filter: settings.filter,
      permissions: group == .permissions ? permissions(settings) : nil,
      shortcuts: shown == .general ? Hotkeys.shortcuts : [])
  }

  private func permissions(_ settings: SettingsStore) -> SettingsScreen.Permissions {
    SettingsScreen.Permissions(
      rules: (settings.view?.rules ?? []).map {
        .init(kind: Self.kind($0.kind), text: $0.rule, source: Self.source($0.layer))
      },
      grants: (settings.view?.grants ?? []).map { grant in
        .init(
          id: grant.id, subject: "\(grant.tool) \(grant.subject)",
          detail: grant.title.map { "in “\($0)”" } ?? "this session")
      },
      failure: settings.ruleFailure)
  }

  private func field(_ field: SettingsField) -> SettingsScreen.Field {
    let control: SettingsScreen.Control =
      switch field.control {
      case .toggle(let isOn): .toggle(isOn)
      case .slider(let value, let range, let text):
        dragged[field.id].map { .slider($0, range: range, text: $0.formatted()) }
          ?? .slider(value, range: range, text: text)
      case .choice(let value, let options): .choice(value, options: options)
      case .menu(let value, let options):
        .menu(value, options: options.map { .init(value: $0.value, title: $0.title) })
      case .field(let text): .field(text)
      case .json(let text): .json(text)
      }
    return SettingsScreen.Field(
      id: field.id, title: field.title, detail: field.detail,
      source: Self.source(field.setting.layer), control: control)
  }

  private func handle(_ intent: SettingsScreenIntent, _ settings: SettingsStore) {
    switch intent {
    case .select(let selected): page = selected
    case .filter(let query): settings.filter = query
    case .set(let key, .number(let value)):
      dragged[key] = value
      sliderWrites.submit(key) { await settings.edit(key, .number(value)) }
    case .set(let key, .bool(let isOn)): Task { await settings.edit(key, .bool(isOn)) }
    case .set(let key, .text(let text)): Task { await settings.edit(key, .text(text)) }
    case .storeKey(let provider, let secret):
      do {
        try settings.storeKey(secret, for: provider)
      } catch {
        refused = String(describing: error)
      }
    case .setLogin(let server, let login): Task { await settings.setLogin(server, login) }
    case .editRule, .revokeGrant: handlePermissions(intent, settings)
    }
  }

  /// The Permissions page's rule edits and revokes; Rust checks and applies both.
  private func handlePermissions(_ intent: SettingsScreenIntent, _ settings: SettingsStore) {
    switch intent {
    case .editRule(let kind, let old, let new):
      Task { await settings.editRule(Self.kind(kind), old: old, new: new) }
    case .revokeGrant(let id):
      if let grant = settings.view?.grants.first(where: { $0.id == id }) {
        Task { await settings.revoke(grant) }
      }
    default: break
    }
  }

  private func action(_ row: McpLoginRow) -> LoginAction? {
    row.action.map { $0 == .logIn ? .logIn : .logOut }
  }

  private static func kind(_ kind: RuleKind) -> SettingsScreen.RuleKind {
    switch kind {
    case .allow: .allow
    case .ask: .ask
    case .deny: .deny
    }
  }

  private static func kind(_ kind: SettingsScreen.RuleKind) -> RuleKind {
    switch kind {
    case .allow: .allow
    case .ask: .ask
    case .deny: .deny
    }
  }

  private static func status(_ status: McpStatus) -> ServerStatus {
    switch status {
    case .connected: .connected
    case .needsLogin: .needsLogin
    case .failed: .failed
    case .disabled: .disabled
    case .unknown: .unknown
    }
  }

  private static func source(_ layer: Layer) -> SettingSource {
    switch layer {
    case .default: .default
    case .user: .user
    case .project: .project
    case .claudeSettings: .claudeSettings
    case .env: .env
    case .flag: .flag
    }
  }
}
