// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A `PlatformHost` as the generated `AppHost` (DT§4.4): the app passes
// `HostBridge(MacHost())` to `LiveCoreClient`. Separate from the client
// because it runs the other way — Rust calls it — and it is the one place
// cox-ffi's inbox item becomes CoxClient's, which `HostNote` is made from.

import CoxClient
import CoxFFIBindings
import Foundation

public final class HostBridge: AppHost {
  private let host: any PlatformHost

  public init(_ host: any PlatformHost) { self.host = host }

  public func notify(item: CoxFFIBindings.InboxItem, badge: UInt32) {
    host.notify(HostNote(CoxClient.InboxItem(item), badge: Int(badge)))
  }

  public func badge(badge: UInt32) { host.badge(Int(badge)) }

  public func openUrl(url: String) { host.open(url) }

  public func confirmOpenUrl(origin: String, url: String) { host.confirmOpen(url, from: origin) }

  public func secret(section: String) -> String? { host.secret(for: section) }

  public func hasBrowser() -> Bool { host.hasBrowser }

  public func browserLoad(url: String) async throws {
    do throws(CoxClient.BrowserFailure) {
      try await host.browserLoad(url)
    } catch {
      throw CoxFFIBindings.BrowserFailure(error)
    }
  }

  public func browserText() async throws -> CoxFFIBindings.PageText {
    do throws(CoxClient.BrowserFailure) {
      let page = try await host.browserText()
      return CoxFFIBindings.PageText(title: page.title, url: page.url, text: page.text)
    } catch {
      throw CoxFFIBindings.BrowserFailure(error)
    }
  }

  public func browserSnapshot() async throws -> Data {
    do throws(CoxClient.BrowserFailure) {
      return Data(try await host.browserSnapshot())
    } catch {
      throw CoxFFIBindings.BrowserFailure(error)
    }
  }
}

extension CoxFFIBindings.BrowserFailure {
  /// UniFFI lifts only its own error type back into Rust; any other would
  /// arrive as an unexpected callback error.
  init(_ failure: CoxClient.BrowserFailure) {
    switch failure {
    case .noPage: self = .NoPage
    case .page(let message): self = .Page(message: message)
    }
  }
}

extension CoxClient.InboxItem {
  init(_ item: CoxFFIBindings.InboxItem) {
    let need: CoxClient.Need =
      switch item.need {
      case .approval(let call, let why):
        .approval(call: call.id, tool: call.name, subject: call.subject, why: .init(why))
      case .question(let call, let question, let options):
        .question(call: call, question: question, options: options)
      case .failed(let text): .failed(text: text)
      case .taskDone(let task, let label, let succeeded):
        .taskDone(task: task, label: label, succeeded: succeeded)
      }
    self.init(
      session: item.session,
      source: item.source.map { .init(session: $0.session, agent: $0.agent, preset: $0.preset) },
      need: need, expired: item.expired, seq: item.seq, title: item.title,
      subtitle: item.subtitle, status: .init(item.status))
  }
}

extension CoxClient.InboxStatus {
  init(_ status: CoxFFIBindings.InboxStatus) {
    self =
      switch status {
      case .waiting: .waiting
      case .idle: .idle
      case .error: .error
      }
  }
}
