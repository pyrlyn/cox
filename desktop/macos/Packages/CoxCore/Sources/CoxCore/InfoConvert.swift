// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Info tab's record from cox-ffi into CoxClient's (T37.29.5), apart from `Convert.swift`'s
// timeline so each file stays one concern. Field for field; nothing is decided here.

import CoxClient
import CoxFFIBindings

extension CoxClient.Info {
  init(_ info: CoxFFIBindings.Info) {
    self.init(
      session: info.session, cwd: info.cwd,
      worktree: info.worktree.map { CoxClient.Linked($0) },
      config: info.config.map {
        CoxClient.ConfigSource(layer: .init($0.layer), file: $0.file, keys: $0.keys)
      },
      rollout: info.rollout, facts: info.facts.map { CoxClient.Fact($0) },
      configFacts: info.configFacts.map { CoxClient.Fact($0) })
  }
}

extension CoxClient.Fact {
  /// Shared by the Info and Changes tabs.
  init(_ fact: CoxFFIBindings.Fact) {
    self.init(label: fact.label, value: fact.value, detail: fact.detail)
  }
}
