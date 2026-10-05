// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the Settings screen (T37.30.1): mockup screen 18's Models &
// Providers page — a provider's box with its key field, and a tier the project's config sets —
// an Appearance page with a choice, a slider and a switch, and screen 19's Permissions page.
// Separate from `PreviewState+Settings.swift`, which holds the setting molecules' fixtures, so
// each card adds its fixtures without editing another's file.

import SwiftUI

extension PreviewState {
  static let userFile = "~/.cox/config.toml"
  static let projectFile = "~/code/cox/.cox/config.toml"
  static let boxTitle = "tiers.code"
  static let fieldText = "https://api.anthropic.com"
  static let fieldPrompt = "Base url"
  static let keyPrompt = "Add key"

  /// The Models & Providers page with no key stored and the code tier set by the project.
  static let settingsModels = SettingsScreenState(
    pages: SettingsPage.allCases, selection: .models,
    tables: [
      .init(
        id: "providers.anthropic",
        fields: [field("providers.anthropic.base_url", .user, .field(fieldText))],
        key: .init(provider: "anthropic", isStored: false)),
      .init(
        id: "tiers.code",
        fields: [
          field(
            "tiers.code.model", .project, .field("claude-sonnet-5"),
            detail: "Set in \(projectFile)"),
          field(
            "tiers.code.effort", .default, .choice("high", options: ["low", "medium", "high"]),
            detail: "How hard the code tier thinks"),
        ]),
    ],
    userFile: userFile, projectFile: projectFile)

  /// The Appearance page: the material the user picked, the opacity left at its default and a
  /// switch set by the environment.
  static let settingsAppearance = SettingsScreenState(
    pages: SettingsPage.allCases, selection: .appearance,
    tables: [
      .init(
        id: "desktop.appearance",
        fields: [
          field(
            "desktop.appearance.material", .user,
            .choice("frosted", options: ["frosted", "glossy", "solid"])),
          field("desktop.appearance.opacity", .default, .slider(0.42, range: 0...1, text: "0.42")),
          field("desktop.appearance.tint", .env, .toggle(true), detail: "Tint from wallpaper"),
        ])
    ],
    userFile: userFile)

  /// Mockup screen 19's Permissions page: the mode and approval policy at their defaults, a
  /// switch the user set, and the rules as the list Settings shows but does not edit.
  static let settingsPermissions = SettingsScreenState(
    pages: SettingsPage.allCases, selection: .permissions,
    tables: [
      .init(
        id: "permissions",
        fields: [
          field(
            "permissions.mode", .default,
            .choice("default", options: ["default", "plan", "auto"]),
            detail: "For new sessions"),
          field(
            "permissions.approval", .default,
            .choice("on-request", options: ["on-request", "never"])),
          field(
            "permissions.allow_for_session_persists", .user, .toggle(false),
            detail: "Whether an allow-for-session grant outlives the session"),
          field("permissions.deny", .default, .json(#"["Read(~/.ssh/**)", "Bash(rm -rf /*)"]"#)),
        ])
    ],
    userFile: userFile, projectFile: projectFile)

  static func field(
    _ key: String, _ source: SettingSource, _ control: SettingsScreen.Control,
    detail: String? = nil
  ) -> SettingsScreen.Field {
    let name = (key.split(separator: ".").last.map(String.init) ?? key)
      .replacingOccurrences(of: "_", with: " ")
    return .init(
      id: key, title: name.prefix(1).uppercased() + name.dropFirst(), detail: detail,
      source: source, control: control)
  }
}
