// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One window's sessions (DT§5.1): opens a session on the launch's core and shows it in CoxUI's
// `MainScreen`, the transcript and composer in its column, over the behind-window blur; the
// first-run checklist comes first on a first launch (DT§5.8), after the login shell's
// environment is read (DT§4.8). Wiring only — the stores decide and the packages draw. The
// sidebar lists the workspace's sessions and opens one here, the toolbar shows the open one's
// title, model, mode and cost and stops its turn, the toolbar's title and a sidebar row's menu
// rename a session (A113), its model popover switches the session's model
// as `/model` does, the inspector's tabs read the open session,
// Review replaces the transcript column, the shell's panes fold, ⌃` shows the session's terminal
// pane under the column (T51.6; the window asks before closing over a running command), ⌘⇧B
// shows the browser pane beside it (T51.10), plugin panels sit above the composer and a plugin
// overlay shows as a sheet (T52.17), File › Connect to Host… lists a remote host's sessions in
// the sidebar and opens one through that host (T52.21), New session asks which agent drives it
// and an external agent's transcript opens with its ACP banner (T52.8), the composer offers
// best of n and its compare sheet (T52.12), the Appearance popover writes
// `[desktop.appearance]`, and ⌘K lays the command palette over the window (T37.44.13).
// A popped-out window (T51.11) is the same view on one session with no sidebar; every window
// on a session shares its stores through `AppStore`.

import CoxClient
import CoxModel
import CoxTranscript
import CoxUI
import SwiftUI

struct SessionWindow: View {
  let model: AppModel
  /// Set for a popped-out window: the one session it shows, and the window it joins as a tab.
  let popOut: PopOut?
  /// The syntax theme the fixtures were recorded with; Settings' appearance replaces it.
  static let syntaxTheme = "base16-ocean.dark"
  /// This window's hold on the sessions it shows in `AppStore`.
  @State private var windowID = UUID()
  /// Set once first run chose a project; a fixture launch never asks.
  @AppStorage("CoxOnboarded") private var onboarded = false
  @State var screen = MainScreenState()
  @State var appearanceWrites = Coalescer()
  /// Every session this window opened, by id; each keeps pulling while another shows.
  @State var opened: [String: OpenedSession] = [:]
  /// The one the window shows.
  @State var current: String?
  @State private var isOpening = false
  /// The login shell's environment is in the process, so sessions may open.
  @State private var isEnvLoaded = false
  /// The checklist's provider-key row, for the sidebar's footer dot.
  @State private var providerCheck: CheckRow?
  /// The providers a turn could run on now, for the footer's count (A110); nil until probed.
  @State private var usable: [String]?
  /// Review shows in the column instead of the transcript, at this file or the first changed one.
  @State var reviewing: Reviewing?
  /// Why the first session did not open.
  @State private var failure: String?
  /// Why the core refused the last intent; shown until dismissed.
  @State var refused: String?
  /// The terminal pane shows under the column; its height is the user's drag, UI-only.
  @State var isTerminalVisible = false
  @State private var terminalHeight = SessionTerminal.defaultHeight
  /// The browser pane shows beside the column; UI-only, like the terminal's.
  @State var isBrowserVisible = false
  /// ⌘K's palette, while it shows (T37.44.13).
  @State var palette: SessionPalette?
  /// The Connect to Host sheet, while it shows (T52.21).
  @State var connecting: ConnectHostSheet.State?
  /// The New-session sheet's agents, while it shows (T52.8).
  @State private var picking: AgentPicker?
  /// The composer's best-of-n candidates and the group the compare sheet shows (T52.12).
  @State private var bestOf = BestOfLauncher()
  @Environment(\.coxAppearance) private var base
  @Environment(\.openWindow) private var openWindow

  init(model: AppModel, popOut: PopOut? = nil) {
    self.model = model
    self.popOut = popOut
    var screen = MainScreenState()
    screen.isSidebarVisible = popOut == nil
    _screen = State(initialValue: screen)
  }

  var body: some View {
    Group {
      if !isEnvLoaded {
        ProgressView().task {
          await model.loadLoginEnv()
          isEnvLoaded = true
        }
      } else if isFirstRun {
        FirstRun(launch: model.launch) { onboarded = true }
      } else {
        main
      }
    }
    .environment(\.coxAppearance, screen.appearance.applied(to: base))
    .behindWindowBlur(
      screen.appearance.blurFraction, tint: screen.appearance.tint,
      in: RoundedRectangle(cornerRadius: Radius.window, style: .continuous)
    )
    // First run has no sidebar: its buttons sit on its window pane's top row.
    .seeThroughWindow(paneTop: isEnvLoaded && isFirstRun ? 0 : Size.paneGap)
    .task { await readSettings() }
    .onChange(of: model.settings?.view) {
      if !appearanceWrites.isPending { readAppearance() }
    }
    .alert(refused ?? "", isPresented: isRefused) {}
    .connectHostSheet($connecting, remotes: model.remotes)
    .newSessionSheet($picking) { agent in await open(resume: nil, agent: agent) }
    .bestOfSheet(bestOf, workspace: try? model.launch.live.get()) { session in
      handle(Sidebar.Intent.select(session))
      reviewing = Reviewing(path: nil)
    }
  }

  private var main: some View {
    MainScreen(state: shown, send: handle, transcript: { column }, inspector: { inspector($0) })
      .focusedSceneValue(
        \.shell,
        ShellActions(
          isSidebarVisible: screen.isSidebarVisible, isInspectorVisible: screen.isInspectorVisible,
          isTerminalVisible: isTerminalShown, isBrowserVisible: isBrowserVisible,
          toggleSidebar: { toggleSidebar() },
          toggleInspector: { screen.isInspectorVisible.toggle() },
          toggleTerminal: { toggleTerminal() }, toggleBrowser: { isBrowserVisible.toggle() },
          popOut: current.map { session -> (Bool) -> Void in { openPopOut(session, asTab: $0) } },
          connectHost: model.remotes.canConnect ? { connecting = ConnectHostSheet.State() } : nil,
          newSession: { Task { await newSession() } }, review: showing.map { _ in toggleReview },
          palette: showing.map { _ in togglePalette })
      )
      .commandPalette(palette?.state, send: handle)
      .task { if current == nil { await open(resume: popOut?.session) } }
      .task { await watch() }
      .task { if popOut == nil { await model.remotes.watch() } }
      .onDisappear {
        for session in opened.values { session.close(in: model.registry, window: windowID) }
      }
      .joinsTabs(of: popOut?.tabOf)
      .closeGuard { opened.values.contains { $0.store.hasBusyTerminal } }
      // The browser takes the inspector's place beside the column, as mockup 25 draws it: with
      // both, the panes outgrow the window and push the toolbar's trailing buttons off its edge.
      .onChange(of: isBrowserVisible) { if isBrowserVisible { screen.isInspectorVisible = false } }
      .onChange(of: screen.isInspectorVisible) { _, shown in if shown { isBrowserVisible = false } }
  }

  private var shown: MainScreenState {
    var state = screen
    state.toolbar = ShellState.toolbar(showing, sidebar: model.sidebar, popover: screen.popover)
    state.model = ShellState.models(showing?.menu)
    state.sidebar = ShellState.sidebar(
      model.sidebar, remotes: model.remotes, selection: current,
      providers: ProviderHealth(usable: usable, check: providerCheck))
    return state
  }

  var showing: OpenedSession? { current.flatMap { opened[$0] } }

  private var isFirstRun: Bool { !onboarded && !model.launch.isFixture }

  /// The pane shows while it is toggled on and the session has a terminal left open.
  var isTerminalShown: Bool {
    isTerminalVisible && showing?.store.terminals.isEmpty == false
  }

  /// A popped-out window has no sidebar to show.
  func toggleSidebar() {
    if popOut == nil { screen.isSidebarVisible.toggle() }
  }

  /// Opens `session` in a window of its own, or as a tab of this one.
  func openPopOut(_ session: String, asTab: Bool) {
    openWindow(value: PopOut(session: session, asTab: asTab))
  }

  private var isRefused: Binding<Bool> {
    Binding(get: { refused != nil }, set: { if !$0 { refused = nil } })
  }

  @ViewBuilder private var column: some View {
    if let showing, let reviewing {
      SessionReview(
        store: showing.store, path: reviewing.path,
        reviewSend: model.settings?.reviewSend ?? .queue
      ) { refused = $0 }
      .onExitCommand { self.reviewing = nil }
    } else if let showing {
      HStack(spacing: 0) {
        VStack(spacing: 0) {
          if let agent = showing.agent(in: model.sidebar) {
            AcpBanner(agent: agent)
              .frame(maxWidth: Size.readingWidth)
              .padding(.horizontal, Space.xl)
              // Mockup 27: the transcript's 6 over the notice's own 10.
              .padding(.top, Space.xl)
          }
          TranscriptView(store: showing.store, send: send)
            .composer(showing.composer)
            .overlay {
              // Figma frame 22: an empty session shows the welcome hero until its first block.
              if showing.store.blocks.isEmpty, let info = showing.info {
                SessionWelcome(
                  project: screen.toolbar.project, cwd: info.cwd, composer: showing.composer,
                  // cox-app reads the folder (T37.49); without a core the hero asks alone.
                  service: (try? model.launch.live.get()) ?? FixtureWelcome())
              }
            }
          let panels = PluginWidgets.panels(showing.store)
          if !panels.isEmpty { PluginPanel(panels).fixedSize(horizontal: false, vertical: true) }
          BestOfBar(launcher: bestOf, open: showing, model: model) { sessions in
            for session in sessions where opened[session.id] == nil {
              opened[session.id] = OpenedSession(model.registry.adopt(session, window: windowID))
            }
          }
          // At its own height, so the transcript takes the rest of the column.
          SessionComposer(store: showing.composer).fixedSize(horizontal: false, vertical: true)
          if isTerminalShown {
            SessionTerminal(
              store: showing.store, surfaces: showing.terminals,
              branch: showing.info?.worktree?.branch, height: $terminalHeight
            ) { refused = $0 }
          }
        }
        // The column is the plugins' window: a panel lays out across it (PL§8 "resized").
        .onGeometryChange(for: CGSize.self, of: \.size) { size in
          let cells = PluginWidgets.cells(size, scale: base.textScale)
          showing.store.pluginArea(width: cells.width, height: cells.height)
        }
        if isBrowserVisible {
          // Up to the mockup's width, narrower when the window is: at a fixed width a small
          // window's column cannot hold it beside the transcript.
          SessionBrowser(controller: model.launch.browser) { refused = $0 }
            .frame(maxWidth: SessionBrowser.paneWidth)
        }
      }
      // A new view per session, so the transcript's text is rebuilt from the one it shows.
      .id(current)
      .pluginOverlaySheet(showing.store)
    } else if let failure {
      Text(failure).textSelection(.enabled)
    } else {
      ProgressView()
    }
  }

  @ViewBuilder private func inspector(_ tab: InspectorTab) -> some View {
    if let showing {
      SessionInspector(
        store: showing.store, tab: tab, cacheHit: model.settings?.cacheHitScope ?? .turn,
        agents: showing.agents
      ) { request in
        switch request {
        // The tab's Review button shows Review, or hides it again.
        case .review(nil): reviewing = reviewing == nil ? Reviewing(path: nil) : nil
        case .review(let path): reviewing = Reviewing(path: path)
        case .open(let session): handle(Sidebar.Intent.select(session))
        case .refused(let why): refused = why
        }
      }
      .id(current)
    }
  }

  /// Reads the footer's provider health, then keeps the session list current while the window
  /// is open: this and other processes add sessions, and the inbox changes as turns run.
  private func watch() async {
    if let live = try? model.launch.live.get() {
      let cwd = LaunchCore.project()
      providerCheck = try? await live.checklist(cwd: cwd).first { $0.id == .providerKey }
      usable = try? await live.usableProviders(cwd: cwd)
    }
    await model.sidebar.watch()
  }

  private func readSettings() async {
    appearanceWrites.onIdle = { readAppearance() }
    await model.settings?.load()
    readAppearance()
  }

  private func readAppearance() {
    guard let settings = model.settings else { return }
    screen.appearance = AppearancePopover.State(settings)
  }

  /// New session: asks who drives it first when an external agent is configured (T52.8).
  func newSession() async {
    picking = await NewSession.picker(model.launch)
    if picking == nil { await open(resume: nil) }
  }

  /// Opens a new session, driven by `agent` when one was picked, or resumes `resume` where it
  /// last ran, and shows it. A session another window already shows is joined, not opened again.
  func open(resume: String?, agent: String? = nil) async {
    guard !isOpening else { return }
    isOpening = true
    defer { isOpening = false }
    do {
      // A remote host's session opens through that host, in its cwd there (T52.21).
      let remote = resume.flatMap { model.remotes.workspace(for: $0) }
      let cwd =
        remote?.cwd ?? resume.flatMap { model.sidebar.entry($0)?.session.cwd }
        ?? LaunchCore.project()
      let shared: AppStore.Shared
      if let resume, let joined = model.registry.join(resume, window: windowID) {
        shared = joined
      } else {
        let core: any CoreClient
        if let remote { core = remote.workspace } else { core = try model.launch.core.get() }
        let client = try await core.open(
          OpenSession(cwd: cwd, resume: resume, theme: Self.syntaxTheme, agent: agent))
        shared = model.registry.adopt(client, window: windowID)
      }
      let client = shared.store.session
      // Notification and menu-bar answers find the store by the session's id; registered now,
      // since a remote session has no `info()` to wait for (T52.20).
      model.register(shared.store, as: client.id)
      // An asked session was held for this window (T51.17); the window holds it now.
      if let handoff = popOut?.handoff { model.registry.release(client.id, window: handoff) }
      if opened[client.id] == nil { opened[client.id] = OpenedSession(shared) }
      (current, failure, reviewing) = (client.id, nil, nil)
      // The local config's models; a remote session's cwd is not a path here.
      if remote == nil, let live = try? model.launch.live.get() {
        opened[client.id]?.models = (try? live.models(cwd: cwd)) ?? []
        opened[client.id]?.modelSections = (try? live.modelMenu(cwd: cwd)) ?? []
      }
      model.sidebar.refresh()
      // Loads the granted plugins, so after it shows; a remote cwd is not a path here either.
      if remote == nil, let live = try? model.launch.live.get() {
        opened[client.id]?.agents = (try? await live.agents(cwd: cwd)) ?? []
      }
      // After it shows: Info asks git about the cwd, which can take a while.
      if let info = try? await client.info() { opened[client.id]?.info = info }
    } catch {
      // Without a first session the column says why; later, an alert does.
      if current == nil {
        failure = String(describing: error)
      } else {
        refused = String(describing: error)
      }
    }
  }
}

/// Where Review opened.
struct Reviewing: Equatable {
  var path: String?
}
