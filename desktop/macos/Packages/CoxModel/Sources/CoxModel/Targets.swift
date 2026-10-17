// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The targets File › Connect to Target… offers (DT§5.10, T68.5): the core's list grouped by kind,
// configured ones first, the saved ssh hosts as `ssh:<alias>`, a typed name as a `kind:name`, and
// the `hello` answer as the two lines the sheet shows. Pure values, so a test needs no core. The
// core (cox-target) checks every name again before anything runs; this only keeps an empty or
// space-ridden name from a round trip.

import CoxClient
import Foundation

/// The four kinds of target, in the order the picker shows them.
public enum TargetKind: String, CaseIterable, Sendable {
  case ssh, docker, lima, colima

  public var title: String {
    switch self {
    case .ssh: "SSH"
    case .docker: "Docker"
    case .lima: "Lima"
    case .colima: "Colima"
    }
  }

  /// What the empty name field suggests.
  public var example: String {
    switch self {
    case .ssh: "devbox"
    case .docker: "web"
    case .lima: "default"
    case .colima: "default"
    }
  }

  /// The kind a `kind:name` names; `nil` for a bare name or an unknown kind.
  public init?(target text: String) {
    guard let colon = text.firstIndex(of: ":"), let kind = Self(rawValue: String(text[..<colon]))
    else { return nil }
    self = kind
  }
}

/// One row of the picker.
public struct TargetOption: Identifiable, Equatable, Sendable {
  public var id: String { target }
  public let kind: TargetKind
  public let name: String
  /// What `connect` takes.
  public let target: String
  /// Named in config, as opposed to found running.
  public let isConfigured: Bool
}

/// What `hello` told about the machine, as the sheet words it before a session opens.
public struct HostHello: Equatable, Sendable {
  /// `macOS · arm64 · cox 0.9.1`.
  public let summary: String
  /// The confining backend, or `nil` when the target has none: the session then asks before every
  /// command (D7).
  public let sandbox: String?

  public init(summary: String, sandbox: String?) {
    (self.summary, self.sandbox) = (summary, sandbox)
  }

  public init(_ info: HostInfo) {
    let name =
      switch info.platform {
      case "macos": "macOS"
      case "linux": "Linux"
      case "windows": "Windows"
      default: info.platform
      }
    let arch = info.arch == "aarch64" ? "arm64" : info.arch
    self.init(summary: "\(name) · \(arch) · cox \(info.cox)", sandbox: info.sandbox)
  }
}

public enum TargetList {
  /// `choices` and the saved ssh `hosts` as options: one per target text, grouped by kind in the
  /// picker's order, configured before running within a kind. A choice whose text names no kind
  /// is dropped, since there is nothing to connect it with.
  public static func options(_ choices: [TargetChoice], hosts: [String]) -> [TargetOption] {
    var seen = Set<String>()
    let all =
      choices.map { ($0.name, $0.target, $0.configured) }
      + hosts.map { ($0, sshText($0), true) }
    let unique = all.filter { seen.insert($0.1).inserted }
    return TargetKind.allCases.flatMap { kind in
      let ofKind = unique.compactMap { name, target, configured -> TargetOption? in
        TargetKind(target: target) == kind
          ? TargetOption(kind: kind, name: name, target: target, isConfigured: configured) : nil
      }
      return ofKind.filter(\.isConfigured) + ofKind.filter { !$0.isConfigured }
    }
  }

  /// The target text a typed `name` makes under `kind`; `nil` when it is empty or has a space or
  /// a control character in it.
  public static func text(kind: TargetKind, name: String) -> String? {
    let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
    let bad = name.unicodeScalars.contains {
      CharacterSet.whitespacesAndNewlines.contains($0)
        || CharacterSet.controlCharacters.contains($0)
    }
    return name.isEmpty || bad ? nil : "\(kind.rawValue):\(name)"
  }

  /// A saved ssh host alias as a target: the literal form, so a `[targets.<alias>]` entry of the
  /// same name cannot redirect it.
  public static func sshText(_ alias: String) -> String { "ssh:\(alias)" }

  /// The sidebar group id of a target: an ssh host keeps its bare alias, as before targets.
  public static func hostID(_ text: String) -> String {
    text.hasPrefix("ssh:") ? String(text.dropFirst("ssh:".count)) : text
  }

  /// The target text a sidebar group id stands for; the inverse of `hostID`.
  public static func text(hostID: String) -> String {
    TargetKind(target: hostID) == nil ? sshText(hostID) : hostID
  }
}
