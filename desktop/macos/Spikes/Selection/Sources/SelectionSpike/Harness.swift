// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Headless measuring kit shared by both candidates: a borderless window placed far
// off screen (ordered in, so AppKit and SwiftUI lay out and draw it), synthetic
// mouse events, per-frame timing with CACurrentMediaTime and the process footprint.
// Frame times here are main-thread layout + draw + CATransaction commit per step;
// the render server's GPU time is not in them (see research.md §9.5 table notes).

import AppKit
import QuartzCore

@MainActor
public final class Offscreen {
  public let window: NSWindow

  public init(size: NSSize = NSSize(width: 900, height: 800)) {
    NSApplication.shared.setActivationPolicy(.accessory)
    window = NSWindow(
      contentRect: NSRect(origin: NSPoint(x: -20_000, y: -20_000), size: size),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.orderFrontRegardless()
  }

  /// One frame: layout, draw and commit.
  public func flush() {
    window.layoutIfNeeded()
    window.displayIfNeeded()
    CATransaction.flush()
  }

  public func spin(_ seconds: TimeInterval) {
    RunLoop.main.run(until: Date(timeIntervalSinceNow: seconds))
  }

  /// Spins the run loop (1 ms slices) until `done` holds; false on timeout.
  public func wait(timeout: TimeInterval, until done: () -> Bool) -> Bool {
    let deadline = Date(timeIntervalSinceNow: timeout)
    while Date() < deadline {
      flush()
      if done() { return true }
      spin(0.001)
    }
    return done()
  }

  public func mouse(_ type: NSEvent.EventType, at point: NSPoint, number: Int = 0) -> NSEvent? {
    NSEvent.mouseEvent(
      with: type, location: point, modifierFlags: [],
      timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
      context: nil, eventNumber: number, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)
  }

  /// One continuous drag, each event sent through the window as AppKit does for a real mouse.
  /// TextKit 2's NSTextView and Textual's interaction view both handle `mouseDragged` per
  /// event; neither runs its own tracking loop, so this reaches the same code a hand drag does.
  public func dispatchedDrag(from start: NSPoint, to end: NSPoint, steps: Int = 8) {
    if let down = mouse(.leftMouseDown, at: start) { window.sendEvent(down) }
    for step in 1...steps {
      let t = CGFloat(step) / CGFloat(steps)
      let point = NSPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t)
      if let event = mouse(.leftMouseDragged, at: point, number: step) { window.sendEvent(event) }
    }
    if let up = mouse(.leftMouseUp, at: end, number: steps + 1) { window.sendEvent(up) }
  }

  /// Scrolls `scroll` by `delta` points per frame and times each frame.
  public func measureScroll(_ scroll: NSScrollView, steps: Int, delta: CGFloat) -> FrameStats {
    let clip = scroll.contentView
    var samples: [Double] = []
    samples.reserveCapacity(steps)
    for _ in 0..<steps {
      let start = CACurrentMediaTime()
      var origin = clip.bounds.origin
      origin.y += delta
      clip.scroll(to: origin)
      scroll.reflectScrolledClipView(clip)
      flush()
      samples.append((CACurrentMediaTime() - start) * 1_000)
    }
    return FrameStats(samples)
  }

  public func close() {
    window.orderOut(nil)
    window.close()
  }
}

public struct FrameStats: CustomStringConvertible {
  public static let frameBudgetMs = 1_000.0 / 60.0
  public let samples: [Double]

  public init(_ samples: [Double]) { self.samples = samples.sorted() }

  func percentile(_ p: Double) -> Double {
    guard !samples.isEmpty else { return 0 }
    return samples[min(samples.count - 1, Int(Double(samples.count) * p))]
  }

  /// Time over the 16.7 ms frame budget as a share of the frames' budgeted time — the
  /// headless stand-in for XCTest's hitch-time ratio (DT§1: ≤ 1 %).
  public var hitchRatio: Double {
    let over = samples.reduce(0) { $0 + max(0, $1 - Self.frameBudgetMs) }
    return over / (Double(samples.count) * Self.frameBudgetMs)
  }

  public var description: String {
    String(
      format: "frames=%d p50=%.2fms p95=%.2fms p99=%.2fms max=%.2fms over16.7=%d hitch=%.2f%%",
      samples.count, percentile(0.5), percentile(0.95), percentile(0.99), samples.last ?? 0,
      samples.filter { $0 > Self.frameBudgetMs }.count, hitchRatio * 100)
  }
}

public func milliseconds(since start: CFTimeInterval) -> Double {
  (CACurrentMediaTime() - start) * 1_000
}

/// Physical footprint of this process in MB (what Activity Monitor's "Memory" shows).
public func footprintMB() -> Double {
  var info = task_vm_info_data_t()
  var count = mach_msg_type_number_t(
    MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
  let result = withUnsafeMutablePointer(to: &info) {
    $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
      task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
    }
  }
  return result == KERN_SUCCESS ? Double(info.phys_footprint) / 1_048_576 : -1
}

public func descendants<T: NSView>(of view: NSView, as type: T.Type) -> [T] {
  var found: [T] = []
  var stack: [NSView] = [view]
  while let next = stack.popLast() {
    if let match = next as? T { found.append(match) }
    stack.append(contentsOf: next.subviews)
  }
  return found
}

public func measure(_ label: String, _ value: String) {
  print("MEASURE \(label): \(value)")
}
