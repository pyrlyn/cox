// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The shared pointer rule over cox-ffi (A141, T37.51): `cox_render::pointer` decides what the
// pointer looks like over an element, and this only converts. CoxCore hands the app a plain Swift
// answer because `CoxFFIBindings` stays inside this package.

import CoxFFIBindings

/// The core's `Pointer`, as the app maps it to a platform pointer.
public enum CorePointer: Equatable, Sendable {
  case `default`, action, text, notAllowed
  case resize(Edge)

  /// Which way a drag handle moves.
  public enum Edge: Equatable, Sendable {
    case vertical, horizontal
  }
}

/// `cox_render::pointer::pointer`: blocked, then pressed, then clickable, then selectable.
public func corePointer(
  clickable: Bool, selectable: Bool, pressed: Bool, blocked: Bool
) -> CorePointer {
  CorePointer(
    pointer(
      interaction: Interaction(
        clickable: clickable, selectable: selectable, pressed: pressed, blocked: blocked)))
}

extension CorePointer {
  init(_ pointer: Pointer) {
    switch pointer {
    case .default: self = .default
    case .action: self = .action
    case .text: self = .text
    case .notAllowed: self = .notAllowed
    case .resize(let edge): self = .resize(edge == .vertical ? .vertical : .horizontal)
    }
  }
}
