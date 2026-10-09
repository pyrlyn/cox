// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The one snapshot harness every CoxUI suite shares (DS§9): a sample drawn over a colourful
// backdrop in one light/dark × Solid/Frosted cell, through a window-hosted `NSHostingView`
// (`ImageRenderer` drops glass content) into a bitmap of a fixed 2× scale, so the image does
// not depend on the machine's display. Separate so no suite carries its own renderer.
//
// The bitmap is the frame after AppKit has committed the text field, not the first display.
// An `NSTextField` draws once on the cell's default baseline and again once SwiftUI has
// applied the token font, the exact line box and (for the command palette) focus. Which of
// those two landed in `cacheDisplay` depended on load, so the same suite failed on one run
// and passed on the next: the palette's empty placeholder moved down and lost its commas,
// and a rule's mono text moved by a point. The thresholds below stay tight on purpose; they
// reject that jump instead of absorbing it.

import AppKit
import QuartzCore
import SnapshotTesting
import SwiftUI
import Testing

@testable import CoxUI

/// One cell of the snapshot matrix.
struct Variant: CustomTestStringConvertible, Sendable {
  let scheme: ColorScheme
  let material: GlassMaterial

  static let all = [ColorScheme.light, .dark].flatMap { scheme in
    [GlassMaterial.solid, .frosted].map { Variant(scheme: scheme, material: $0) }
  }

  var name: String { "\(scheme == .dark ? "dark" : "light")-\(material.rawValue)" }
  var testDescription: String { name }
}

/// A sample hosted in a borderless window, re-rendered as its root view changes.
@MainActor
struct SnapshotHost<Sample: View> {
  private let settings: Settings
  private let host: NSHostingView<AnyView>
  private let window: NSWindow

  init(
    _ sample: Sample, _ variant: Variant, reduceMotion: Bool = false,
    reduceTransparency: Bool = false
  ) {
    SnapshotRendering.arm()
    settings = Settings(
      variant: variant, reduceMotion: reduceMotion, reduceTransparency: reduceTransparency)
    host = NSHostingView(rootView: Self.dressed(sample, settings))
    host.appearance = NSAppearance(named: variant.scheme == .dark ? .darkAqua : .aqua)
    host.frame = CGRect(origin: .zero, size: host.fittingSize)
    window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    // A shadow and an implicit animation would move pixels the view does not own.
    window.animationBehavior = .none
    window.hasShadow = false
    window.isOpaque = true
    window.contentView = host
    host.layoutSubtreeIfNeeded()
  }

  /// Swaps in `sample`, keeping the view's identity so its animations run.
  func update(_ sample: Sample) {
    host.rootView = Self.dressed(sample, settings)
    host.layoutSubtreeIfNeeded()
  }

  /// The current frame as a bitmap of a fixed 2× scale.
  func bitmap() throws -> NSBitmapImageRep {
    let bitmap = try #require(
      NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(host.bounds.width) * 2,
        pixelsHigh: Int(host.bounds.height) * 2, bitsPerSample: 8, samplesPerPixel: 4,
        hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
        bitsPerPixel: 0))
    bitmap.size = host.bounds.size
    host.cacheDisplay(in: host.bounds, to: bitmap)
    return bitmap
  }

  /// The first frame for which `done` holds, rendered as the main run loop turns. The wait
  /// ends on what is drawn, not on a clock, so load can slow a test but not change the frame
  /// it reads. `limit` only bounds a wait that would never end: then the last frame comes back
  /// and the caller's expectation fails on it.
  func bitmap(
    until done: (NSBitmapImageRep) throws -> Bool, limit: TimeInterval = 10
  ) throws -> NSBitmapImageRep {
    let deadline = Date().addingTimeInterval(limit)
    var frame = try bitmap()
    while try !done(frame), Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(Motion.durationFast))
      frame = try bitmap()
    }
    return frame
  }

  /// The current frame as an image, for `assertSnapshot`. Waits out the text field's
  /// deferred baseline (see `settledBitmap`) so two runs of one suite draw the same pixels.
  func image() throws -> NSImage {
    let bitmap = try settledBitmap()
    let image = NSImage(size: bitmap.size)
    image.addRepresentation(bitmap)
    return image
  }

  /// The first frame that survives a later turn unchanged. SwiftUI commits a text field's
  /// font, line box and focus on the main queue after the first layout, and `cacheDisplay`
  /// sometimes drains that queue and sometimes does not. Drawing until two consecutive
  /// frames match takes the committed one. The budget is short so a spinner that ignored
  /// Reduce Motion cannot travel; past it, the last frame is returned and the caller's
  /// expectation fails on it.
  func settledBitmap() throws -> NSBitmapImageRep {
    SnapshotRendering.arm()
    var previous = try bitmap()
    for _ in 0..<8 {
      RunLoop.main.run(until: Date().addingTimeInterval(1.0 / 60.0))
      host.layoutSubtreeIfNeeded()
      let next = try bitmap()
      if let before = previous.tiffRepresentation, before == next.tiffRepresentation {
        return next
      }
      previous = next
    }
    return previous
  }

  private struct Settings {
    let variant: Variant
    let reduceMotion: Bool
    let reduceTransparency: Bool
  }

  private static func dressed(_ sample: Sample, _ settings: Settings) -> AnyView {
    AnyView(
      sample
        .padding(Space.xxl)
        .background(PreviewBackdrop())
        .environment(\.colorScheme, settings.variant.scheme)
        .environment(\.coxAppearance, Appearance(material: settings.variant.material))
        .environment(\._accessibilityReduceMotion, settings.reduceMotion)
        .environment(\._accessibilityReduceTransparency, settings.reduceTransparency)
        // The runner's locale, clock and text size are not part of the fixture.
        .environment(\.locale, Locale(identifier: "en_US"))
        .environment(\.timeZone, TimeZone(identifier: "UTC") ?? .current)
        .environment(\.layoutDirection, .leftToRight)
        .environment(\.legibilityWeight, .regular)
        .dynamicTypeSize(.large)
        .transaction { $0.disablesAnimations = true })
  }
}

/// Process-wide stills for the snapshot process. The caret blink and Core Animation actions
/// are not part of any fixture; without this, two frames of one field never match.
@MainActor
private enum SnapshotRendering {
  private static var armed = false

  static func arm() {
    guard !armed else { return }
    armed = true
    // Milliseconds, read by `NSTextView` when the field editor is created. A long on-period
    // holds the caret in the phase it starts in. An off-period of 0 is ignored, so the off
    // phase is one millisecond.
    let defaults = UserDefaults.standard
    defaults.set(3_600_000, forKey: "NSTextInsertionPointBlinkPeriodOn")
    defaults.set(1, forKey: "NSTextInsertionPointBlinkPeriodOff")
    CATransaction.setDisableActions(true)
  }
}

/// Asserts `sample` in `variant` against the image `name` of the calling test, stored beside
/// the calling test file.
@MainActor
func assertCoxSnapshot(
  _ sample: some View, _ variant: Variant, reduceTransparency: Bool = false,
  named name: String, testName: String = #function, filePath: StaticString = #filePath,
  fileID: StaticString = #fileID, line: UInt = #line
) throws {
  // Reduce Motion holds a running spinner upright. The settle below drains the main queue,
  // and a `TimelineView` would otherwise advance by however long that drain took.
  let image = try SnapshotHost(
    sample, variant, reduceMotion: true, reduceTransparency: reduceTransparency
  ).image()
  // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
  assertSnapshot(
    of: image, as: .image(precision: 0.995, perceptualPrecision: 0.98), named: name,
    fileID: fileID, file: filePath, testName: testName, line: line)
}

/// Asserts `screen` at `size` in `variant` as a whole window at 1× — its structure is the check,
/// and the organisms' own snapshots hold the detail at 2× — so the committed images stay a few
/// hundred kilobytes each.
@MainActor
func assertCoxWindowSnapshot(
  _ screen: some View, _ variant: Variant, size: CGSize = PreviewState.window,
  reduceTransparency: Bool = false, named name: String? = nil, testName: String = #function,
  filePath: StaticString = #filePath, fileID: StaticString = #fileID, line: UInt = #line
) throws {
  let sample = screen.frame(width: size.width, height: size.height)
  let full = try SnapshotHost(
    sample, variant, reduceMotion: true, reduceTransparency: reduceTransparency
  ).settledBitmap()
  let rep = try #require(
    NSBitmapImageRep(
      bitmapDataPlanes: nil, pixelsWide: Int(full.size.width), pixelsHigh: Int(full.size.height),
      bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
      colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
  rep.size = full.size
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  full.draw(in: CGRect(origin: .zero, size: full.size))
  NSGraphicsContext.restoreGraphicsState()
  let image = NSImage(size: rep.size)
  image.addRepresentation(rep)
  assertSnapshot(
    of: image, as: .image(precision: 0.995, perceptualPrecision: 0.98),
    named: name ?? variant.name,
    fileID: fileID, file: filePath, testName: testName, line: line)
}

/// The harness's claim: once a text field has committed its font and focus, another turn
/// does not move it. This is the jump behind `commandPalette open.*` and the permissions
/// rules, which used to depend on whether `cacheDisplay` drained the main queue.
@MainActor
@Suite struct SnapshotStabilityTests {
  @Test func anEmptyFieldStaysPutOnceTheFrameHasSettled() throws {
    let host = SnapshotHost(
      emptyField, Variant(scheme: .light, material: .solid), reduceMotion: true)
    let settled = try host.settledBitmap()
    RunLoop.main.run(until: Date().addingTimeInterval(1.0 / 60.0))
    let again = try host.bitmap()
    #expect(settled.tiffRepresentation == again.tiffRepresentation)
  }

  @Test func theOpenPaletteStaysPutOnceTheFrameHasSettled() throws {
    let sample = PreviewPane {
      CommandPalette(state: PreviewState.paletteOpen) { _ in }.fixedSize()
    }
    let host = SnapshotHost(sample, Variant(scheme: .dark, material: .frosted), reduceMotion: true)
    let first = try host.settledBitmap()
    let second = try host.settledBitmap()
    #expect(first.tiffRepresentation == second.tiffRepresentation)
  }

  private var emptyField: some View {
    TextField("Search actions, sessions, commands and files", text: .constant(""))
      .textFieldStyle(.plain)
      .textStyle(.titlePalette)
      .frame(width: Size.paletteWidth)
  }
}
