// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session status the composer's chips show (T37.24.7), generated cox-ffi value → `CoxClient`
// value, with the mode and effort enums it carries. Beside `Convert.swift`, which holds every
// other conversion, only to keep that file within the length limit.

import CoxClient
import CoxFFIBindings

extension CoxClient.Status {
  init(_ value: CoxFFIBindings.Status) {
    self.init(
      queued: value.queued, mode: value.mode.map { .init($0) },
      nextMode: value.nextMode.map { .init($0) }, model: value.model,
      effort: value.effort.map { .init($0) }, modelName: value.modelName,
      shortName: value.shortName)
  }
}

extension CoxClient.PermissionMode {
  init(_ value: CoxFFIBindings.PermissionMode) {
    switch value {
    case .default: self = .default
    case .plan: self = .plan
    case .auto: self = .auto
    case .bypass: self = .bypass
    }
  }
}

extension CoxClient.Effort {
  init(_ value: CoxFFIBindings.Effort) {
    switch value {
    case .low: self = .low
    case .medium: self = .medium
    case .high: self = .high
    case .xhigh: self = .xhigh
    }
  }
}
