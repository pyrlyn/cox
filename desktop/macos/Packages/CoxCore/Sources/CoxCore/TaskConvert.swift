// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A task's kind, state and what opening it shows, from cox-ffi into CoxClient's (T37.29.6,
// T58.4.24), apart from `Convert.swift`'s timeline so each file stays one concern. Case for case;
// nothing is decided here.

import CoxClient
import CoxFFIBindings

extension CoxClient.TaskKind {
  init(_ value: CoxFFIBindings.TaskKind) {
    switch value {
    case .agent: self = .agent
    case .shell: self = .shell
    }
  }
}

extension CoxClient.TaskTarget {
  init(_ value: CoxFFIBindings.TaskTarget) {
    switch value {
    case .transcript(let session): self = .transcript(session: session)
    case .output(let archive): self = .output(archive: archive)
    }
  }
}

extension CoxClient.TaskState {
  init(_ value: CoxFFIBindings.TaskState) {
    switch value {
    case .running: self = .running
    case .succeeded: self = .succeeded
    case .failed: self = .failed
    }
  }
}
