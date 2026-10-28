import AppKit
import ApplicationServices
import SwiftUI
import TildeFFI

/// Settings (⌘,): a view of config.toml. Every change is written to the file straight
/// away (keeping its comments) and applied to open windows, so the file stays the one
/// place settings live.
@MainActor
final class SettingsWindowController: NSWindowController {
    private let model: SettingsModel

    init(onChange: @escaping @MainActor () -> Void, showIndex: @escaping @MainActor () -> Void) {
        model = SettingsModel(onChange: onChange, showIndex: showIndex)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 560),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Settings"
        window.minSize = NSSize(width: 560, height: 420)
        window.contentViewController = NSHostingController(rootView: SettingsView(model: model))
        window.setContentSize(NSSize(width: 680, height: 560))
        window.center()
        window.setFrameAutosaveName("TildeSettings")
        super.init(window: window)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Shows the window, re-reading the config (it may have been edited by hand).
    func show(tab: SettingsTab? = nil) {
        model.refresh()
        if let tab { model.tab = tab }
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }
}

enum SettingsTab: Hashable {
    case general, preview, editing, search, cleanup, keys
}

@MainActor
@Observable
final class SettingsModel {
    var settings = loadSettings()
    var keymap = loadKeymap()
    /// Problems in the config file, as the windows report them.
    var configError: String?
    var nvim: (found: Bool, message: String)?
    var tab: SettingsTab = .general
    /// The binding waiting for a key press: (mode, action).
    var recording: (mode: String, action: String, add: Bool)?
    var hasFullDiskAccess = Permissions.hasFullDiskAccess
    var hasAccessibility = AXIsProcessTrusted()
    var recordingHotkey = false
    var hotkeyError: String?
    private let onChange: @MainActor () -> Void
    let showIndex: @MainActor () -> Void
    @ObservationIgnored private var keyMonitor: Any?

    init(onChange: @escaping @MainActor () -> Void, showIndex: @escaping @MainActor () -> Void) {
        self.onChange = onChange
        self.showIndex = showIndex
        configError = loadConfig().error
        checkNvim()
    }

    func refresh() {
        settings = loadSettings()
        keymap = loadKeymap()
        configError = loadConfig().error
        hasFullDiskAccess = Permissions.hasFullDiskAccess
        hasAccessibility = AXIsProcessTrusted()
    }

    /// Waits for the next key press with modifiers and makes it the file-dialog hotkey.
    func recordHotkey() {
        stopRecording()
        recordingHotkey = true
        hotkeyError = nil
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            let spec = DialogHelper.spec(of: event)
            MainActor.assumeIsolated { self?.recordedHotkey(spec) }
            return nil
        }
    }

    private func recordedHotkey(_ spec: String?) {
        guard recordingHotkey else { return }
        stopRecording()
        guard let spec else { return NSSound.beep() }
        do {
            _ = try parseHotkey(spec: spec)
            write("dialog.hotkey", .text(value: spec))
        } catch {
            hotkeyError = error.message
        }
    }

    func write(_ key: String, _ value: SettingValue) {
        do {
            try setSetting(key: key, value: value)
        } catch {
            configError = error.message
            return
        }
        refresh()
        onChange()
    }

    func reset(_ key: String) {
        do {
            try resetSetting(key: key)
        } catch {
            configError = error.message
            return
        }
        refresh()
        onChange()
    }

    func checkNvim() {
        nvim = nil
        let path = settings.nvimPath
        Task {
            let found = await Task.detached {
                let status = findNvim(configured: path)
                return (status.found, status.message)
            }.value
            self.nvim = found
        }
    }

    /// A binding (bool) that writes `key` when changed.
    func bool(_ key: String, _ get: @escaping (TildeFFI.Settings) -> Bool) -> Binding<Bool> {
        Binding(get: { get(self.settings) }, set: { self.write(key, .bool(value: $0)) })
    }

    // MARK: Recording keys

    /// Waits for the next key press and binds it to `action` (replacing its keys, or
    /// adding to them).
    func record(mode: String, action: String, add: Bool) {
        stopRecording()
        recording = (mode, action, add)
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            let spec = Keymap.spec(of: event)
            let used = MainActor.assumeIsolated { self?.recorded(spec) ?? false }
            return used ? nil : event
        }
    }

    /// The key press while recording (`nil`: one that can't be bound, like ⌘K).
    private func recorded(_ spec: String?) -> Bool {
        guard let target = recording else { return false }
        guard let spec else {
            NSSound.beep()
            return true
        }
        let current = (target.mode == "go" ? keymap.go : keymap.select)
            .first { $0.action == target.action }?.keys.map(Self.spec) ?? []
        let keys = target.add ? current + [spec] : [spec]
        stopRecording()
        write("keys.\(target.mode).\(target.action)", .list(value: keys))
        return true
    }

    func stopRecording() {
        recording = nil
        recordingHotkey = false
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
    }

    /// The config spelling of a bound key.
    static func spec(_ k: KeyChord) -> String {
        var parts: [String] = []
        if k.ctrl { parts.append("ctrl") }
        if k.alt { parts.append("alt") }
        if k.shift { parts.append("shift") }
        return (parts + [k.key]).joined(separator: "+")
    }
}

struct SettingsView: View {
    @Bindable var model: SettingsModel

    var body: some View {
        VStack(spacing: 0) {
            if let error = model.configError {
                HStack(alignment: .top) {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    Text(error).font(.callout).textSelection(.enabled)
                    Spacer()
                }
                .padding(10)
                .background(.orange.opacity(0.12))
            }
            TabView(selection: $model.tab) {
                GeneralSettings(model: model).tabItem { Text("General") }.tag(SettingsTab.general)
                PreviewSettings(model: model).tabItem { Text("Preview") }.tag(SettingsTab.preview)
                EditingSettings(model: model).tabItem { Text("Editing") }.tag(SettingsTab.editing)
                SearchSettings(model: model).tabItem { Text("Search") }.tag(SettingsTab.search)
                CleanupSettings(model: model).tabItem { Text("Clean Up") }.tag(SettingsTab.cleanup)
                KeysSettings(model: model).tabItem { Text("Keys") }.tag(SettingsTab.keys)
            }
            .padding(12)
            HStack {
                Text("Saved to \(loadConfig().path.replacingOccurrences(of: NSHomeDirectory(), with: "~"))")
                    .font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Open Config File") { NSApp.sendAction(#selector(AppDelegate.openConfig(_:)), to: nil, from: nil) }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 12)
        }
        .frame(minWidth: 560, minHeight: 420)
        .onDisappear { model.stopRecording() }
    }
}

private struct GeneralSettings: View {
    @Bindable var model: SettingsModel
    private static let families: [String] = {
        // Monospaced families: the list lines up in columns.
        NSFontManager.shared.availableFontFamilies.filter { family in
            guard let member = NSFontManager.shared.availableMembers(ofFontFamily: family)?.first,
                  let name = member.first as? String
            else { return false }
            return NSFont(name: name, size: 12)?.isFixedPitch == true
        }
    }()

    /// Links the command shipped in the app onto the PATH.
    static var linkCommand: String {
        let tool = Bundle.main.bundleURL.appendingPathComponent("Contents/Resources/bin/tilde").path
        return "ln -sf \"\(tool)\" /usr/local/bin/tilde"
    }

    var body: some View {
        Form {
            Section("Appearance") {
                Picker("Appearance", selection: Binding(
                    get: { model.settings.appearance },
                    set: { model.write("theme.appearance", .text(value: Self.name($0))) }
                )) {
                    Text("Follow the system").tag(ThemeAppearance.system)
                    Text("Light").tag(ThemeAppearance.light)
                    Text("Dark").tag(ThemeAppearance.dark)
                }
                Picker("Theme", selection: Binding(
                    get: { model.settings.themePreset },
                    set: { model.write("theme.preset", .text(value: $0)) }
                )) {
                    ForEach(model.settings.presets, id: \.self) { Text($0.capitalized).tag($0) }
                }
                Text("Individual colours can be set in the config file under [theme.colors].")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Font") {
                Picker("Family", selection: Binding(
                    get: { model.settings.fontFamily },
                    set: { model.write("font.family", .text(value: $0)) }
                )) {
                    let current = model.settings.fontFamily
                    if !Self.families.contains(current) { Text(current).tag(current) }
                    ForEach(Self.families, id: \.self) { Text($0).tag($0) }
                }
                Stepper(value: Binding(
                    get: { model.settings.fontSize },
                    set: { model.write("font.size", .float(value: $0)) }
                ), in: 8...32, step: 1) {
                    Text("Size: \(Int(model.settings.fontSize)) pt")
                }
            }
            Section("Activity") {
                Stepper(value: Binding(
                    get: { Int(model.settings.activityDays) },
                    set: { model.write("activity.days", .int(value: Int64($0))) }
                ), in: 1...365) {
                    Text("Show the last \(model.settings.activityDays) days")
                }
            }
            Section("Terminal") {
                Text("The tilde command opens Tilde on a folder from a terminal. With the shell function t, closing the window (q or ⌘W) leaves the terminal in the folder you ended up in.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                CommandRow(label: "Put tilde on your PATH (Homebrew does this)", command: Self.linkCommand)
                CommandRow(label: "Add t to ~/.zshrc", command: #"eval "$(tilde init zsh)""#)
            }
            Section("Folders") {
                Toggle("Sort every folder the same way", isOn: model.bool("sort.same_everywhere") { $0.sortSameEverywhere })
                Text(model.settings.sortSameEverywhere
                    ? "A sort chosen with s applies to every folder you open, and is kept for next time."
                    : "Each folder keeps the sort chosen in it; others use name order.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Toggle("Show folder sizes", isOn: model.bool("sort.folder_sizes") { $0.folderSizes })
                Text("Folders are measured in the background, at low priority, so they show their total size and sort by it. Sizes are remembered between launches.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("File dialogs") {
                Toggle("Search with Tilde in other apps' Open and Save dialogs", isOn: Binding(
                    get: { model.settings.dialogEnabled },
                    set: { on in
                        model.write("dialog.enabled", .bool(value: on))
                        // Ask macOS now, while the reason is on screen.
                        if on, !AXIsProcessTrusted() {
                            Permissions.askForAccessibility()
                        }
                    }))
                LabeledContent("Hotkey") {
                    HStack {
                        if model.recordingHotkey {
                            Text("Press the new hotkey…").foregroundStyle(.tint)
                            Button("Cancel") { model.stopRecording() }
                        } else {
                            Text((try? parseHotkey(spec: model.settings.dialogHotkey))?.display ?? model.settings.dialogHotkey)
                                .font(.system(.body, design: .monospaced))
                            Button("Change") { model.recordHotkey() }
                        }
                    }
                }
                if let error = model.hotkeyError {
                    Text(error).font(.caption).foregroundStyle(.orange)
                }
                Toggle("After picking a file in an Open dialog, press Open", isOn: model.bool("dialog.confirm") { $0.dialogConfirm })
                LabeledContent("Accessibility") {
                    HStack {
                        Text(model.hasAccessibility ? "Allowed" : "Not allowed")
                            .foregroundStyle(model.hasAccessibility ? .green : .orange)
                        Button("Open Privacy & Security") { Permissions.openAccessibilitySettings() }
                    }
                }
                Text("With an Open or Save dialog in front, press the hotkey, pick a file or folder, and the dialog goes there. Accessibility lets Tilde see the dialog and press its keys.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("Permissions") {
                LabeledContent("Full Disk Access") {
                    HStack {
                        Text(model.hasFullDiskAccess ? "Granted" : "Not granted")
                            .foregroundStyle(model.hasFullDiskAccess ? .green : .orange)
                        Button("Open Privacy & Security") { Permissions.openFullDiskAccessSettings() }
                    }
                }
                if Permissions.grantee != "Tilde" {
                    Text("Tilde is running from \(Permissions.grantee), so macOS asks about \(Permissions.grantee).")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
    }

    static func name(_ a: ThemeAppearance) -> String {
        switch a {
        case .system: "system"
        case .light: "light"
        case .dark: "dark"
        }
    }
}

private struct PreviewSettings: View {
    @Bindable var model: SettingsModel
    @State private var width: Double?
    @State private var modeWidth: Double?

    var body: some View {
        Form {
            Section {
                Toggle("Show the preview", isOn: model.bool("preview.visible") { $0.previewVisible })
                LabeledContent("Width") {
                    Slider(value: Binding(get: { width ?? model.settings.previewWidth }, set: { width = $0 }),
                           in: 0.15...0.85) { editing in
                        // Written when the drag ends, not on every step.
                        if !editing, let w = width {
                            model.write("preview.width", .float(value: (w * 100).rounded() / 100))
                            width = nil
                        }
                    }
                    Text("\(Int(((width ?? model.settings.previewWidth) * 100).rounded()))%").monospacedDigit().frame(width: 40)
                }
                .disabled(!model.settings.previewVisible)
                LabeledContent("Width in preview mode (p)") {
                    Slider(value: Binding(get: { modeWidth ?? model.settings.previewModeWidth }, set: { modeWidth = $0 }),
                           in: 0.3...0.95) { editing in
                        if !editing, let w = modeWidth {
                            model.write("preview.mode_width", .float(value: (w * 100).rounded() / 100))
                            modeWidth = nil
                        }
                    }
                    Text("\(Int(((modeWidth ?? model.settings.previewModeWidth) * 100).rounded()))%").monospacedDigit().frame(width: 40)
                }
                .disabled(!model.settings.previewVisible)
                Text("In preview mode, f gives the preview the whole window.")
                    .font(.caption).foregroundStyle(.secondary)
                Toggle("Highlight code and text with bat", isOn: model.bool("preview.bat") { $0.previewBat })
                Text(batInstalled() ? "bat is installed." : "bat isn't installed (brew install bat); Quick Look is used instead.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct EditingSettings: View {
    @Bindable var model: SettingsModel
    @State private var path = ""

    var body: some View {
        Form {
            Section("Edit a folder as text (e, r, o)") {
                Picker("Editor", selection: Binding(
                    get: { model.settings.editorNvim },
                    set: { model.write("edit.editor", .text(value: $0 ? "nvim" : "builtin")) }
                )) {
                    Text("Built-in").tag(false)
                    Text("Neovim").tag(true)
                }
                .pickerStyle(.radioGroup)
            }
            Section("Neovim") {
                LabeledContent("Status") {
                    if let nvim = model.nvim {
                        Label(nvim.message, systemImage: nvim.found ? "checkmark.circle.fill" : "xmark.circle.fill")
                            .foregroundStyle(nvim.found ? .green : .orange)
                            .textSelection(.enabled)
                    } else {
                        ProgressView().controlSize(.small)
                    }
                }
                TextField("Path", text: $path, prompt: Text("Found automatically"))
                    .onSubmit {
                        model.write("edit.nvim_path", .text(value: path.trimmingCharacters(in: .whitespaces)))
                        model.checkNvim()
                    }
                Toggle("Clean config: skip my nvim config, load init.lua next to config.toml",
                       isOn: Binding(
                           get: { model.settings.nvimClean },
                           set: { model.write("edit.nvim_config", .text(value: $0 ? "clean" : "user")) }))
                Text(":w applies, :q discards, ⌘. cancels. Neovim keeps running between edits.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear { path = model.settings.nvimPath }
    }
}

private struct SearchSettings: View {
    @Bindable var model: SettingsModel
    private static let limits: [UInt64] = [250_000, 500_000, 1_000_000, 2_000_000, 5_000_000]

    var body: some View {
        Form {
            Section("Everywhere search") {
                Toggle("Index files for searching everywhere", isOn: model.bool("index.enabled") { $0.indexEnabled })
                Toggle("Include hidden files", isOn: model.bool("index.include_hidden") { $0.indexIncludeHidden })
                Picker("Stop indexing at", selection: Binding(
                    get: { model.settings.indexMaxItems },
                    set: { model.write("index.max_items", .int(value: Int64($0))) }
                )) {
                    let current = model.settings.indexMaxItems
                    if !Self.limits.contains(current) { Text("\(current.formatted()) items").tag(current) }
                    ForEach(Self.limits, id: \.self) { Text("\($0.formatted()) items").tag($0) }
                }
                HStack {
                    Text("Folders, exclusions and hidden items are in the Index window. Changes apply after Rebuild.")
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("Index…") { model.showIndex() }
                }
            }
            Section("Inside files") {
                Toggle("Search inside files with text:", isOn: model.bool("search.contents") { $0.searchContents })
                Toggle("Use Spotlight beyond the open folder", isOn: model.bool("search.spotlight") { $0.searchSpotlight })
                    .disabled(!model.settings.searchContents)
                Text(model.settings.spotlightAvailable
                    ? "Type text:launch or text:\"launch window\" in the bar. The open folder and below are read on request (skipping .gitignored files and build folders; text!: includes them); Spotlight finds the rest, PDFs and documents included."
                    : "Type text:launch or text:\"launch window\" in the bar. The open folder and below are read on request (skipping .gitignored files and build folders; text!: includes them). Spotlight indexing is off on this Mac, so beyond the folder Tilde reads recently changed text files instead.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("Ranking") {
                Toggle("Rank by the folders and files you use most", isOn: model.bool("frecency.enabled") { $0.frecencyEnabled })
                Toggle("Include zoxide's scores (folders you use in the terminal)", isOn: Binding(
                    get: { model.settings.frecencyZoxide },
                    set: { on in
                        // The old [zoxide] setting would win over the new one.
                        try? resetSetting(key: "zoxide.enabled")
                        model.write("frecency.zoxide", .bool(value: on))
                    }))
                    .disabled(!model.settings.frecencyEnabled)
                if !model.settings.zoxideFound {
                    Text("zoxide isn't installed, so only use in Tilde counts.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Toggle("Tell zoxide about folders opened in Tilde (so `z` learns from Tilde)", isOn: Binding(
                    get: { model.settings.frecencyTellZoxide },
                    set: { on in
                        // The old [zoxide] setting would win over the new one.
                        try? resetSetting(key: "zoxide.add_on_visit")
                        model.write("frecency.tell_zoxide", .bool(value: on))
                    }))
                Text("Frequent and recent use counts; a folder you only pass through barely does. Scores halve every 10 days without use.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
    }
}

private struct CleanupSettings: View {
    @Bindable var model: SettingsModel
    @State private var buildFolders: String?

    var body: some View {
        Form {
            Section("Suggestions") {
                choice("Big files: at least", "cleanup.big_file", model.settings.cleanupBigFile,
                       ["100M", "250M", "500M", "1G", "2G", "5G"])
                choice("…not opened for", "cleanup.big_file_age", model.settings.cleanupBigFileAge,
                       ["1mo", "3mo", "6mo", "1y", "2y"])
                choice("Old downloads: older than", "cleanup.download_age", model.settings.cleanupDownloadAge,
                       ["7d", "14d", "30d", "60d", "90d", "6mo", "1y"])
                choice("Build output: projects idle for", "cleanup.project_idle", model.settings.cleanupProjectIdle,
                       ["14d", "30d", "60d", "90d", "6mo", "1y"])
                choice("Duplicates: at least", "cleanup.duplicates_min", model.settings.cleanupDuplicatesMin,
                       ["100K", "1M", "10M", "100M"])
                Text("Changes apply the next time Clean Up looks (r).")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Build folders") {
                TextField("Folder names", text: Binding(
                    get: { buildFolders ?? model.settings.cleanupBuildFolders.joined(separator: ", ") },
                    set: { buildFolders = $0 }))
                    .onSubmit {
                        guard let text = buildFolders else { return }
                        let names = text.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                        model.write("cleanup.build_folders", .list(value: names))
                        buildFolders = nil
                    }
                Text("Folders inside projects (a folder with .git, package.json, Cargo.toml…) that a build makes again. Comma-separated; ⏎ saves.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("Never suggested") {
                if model.settings.cleanupIgnore.isEmpty {
                    Text("Nothing yet. X in Clean Up adds the highlighted item.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                ForEach(model.settings.cleanupIgnore, id: \.self) { entry in
                    HStack {
                        Text(entry).font(.system(.body, design: .monospaced))
                        Spacer()
                        Button("Remove") {
                            model.write("cleanup.ignore", .list(value: model.settings.cleanupIgnore.filter { $0 != entry }))
                        }
                    }
                }
            }
        }
        .formStyle(.grouped)
    }

    /// A picker of common values, plus the current one if it's something else.
    private func choice(_ label: String, _ key: String, _ current: String, _ options: [String]) -> some View {
        Picker(label, selection: Binding(get: { current }, set: { model.write(key, .text(value: $0)) })) {
            if !options.contains(current) { Text(Self.describe(current)).tag(current) }
            ForEach(options, id: \.self) { Text(Self.describe($0)).tag($0) }
        }
    }

    /// "500M" → "500 MB", "6mo" → "6 months".
    static func describe(_ value: String) -> String {
        let units: [(String, String, String)] = [
            ("mo", "month", "months"), ("y", "year", "years"), ("w", "week", "weeks"), ("d", "day", "days"),
            ("h", "hour", "hours"), ("K", "KB", "KB"), ("M", "MB", "MB"), ("G", "GB", "GB"), ("T", "TB", "TB"),
        ]
        for (suffix, one, many) in units where value.hasSuffix(suffix) {
            let number = String(value.dropLast(suffix.count))
            guard Double(number) != nil else { break }
            return "\(number) \(number == "1" ? one : many)"
        }
        return value
    }
}

private struct KeysSettings: View {
    @Bindable var model: SettingsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Click Set and press a key to rebind an action, or + to add another key. GO keys need ⌃ or ⌥ unless they're keys like ↓ or ⇥, since characters are typed into the bar. ⌘ shortcuts are in the menus.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            List {
                Section("GO: typing in the bar") {
                    ForEach(model.keymap.go, id: \.action) { KeyRow(model: model, mode: "go", binding: $0) }
                }
                Section("SELECT: the list") {
                    ForEach(model.keymap.select, id: \.action) { KeyRow(model: model, mode: "select", binding: $0) }
                }
            }
        }
    }
}

private struct KeyRow: View {
    @Bindable var model: SettingsModel
    let mode: String
    let binding: ActionKeys

    private var isRecording: Bool {
        model.recording.map { $0.mode == mode && $0.action == binding.action } ?? false
    }

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 1) {
                Text(binding.description)
                Text(binding.customized ? "\(binding.action) · default \(binding.defaults.joined(separator: " "))" : binding.action)
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if isRecording {
                Text("Press a key…").foregroundStyle(.tint)
                Button("Cancel") { model.stopRecording() }
            } else {
                if binding.keys.isEmpty {
                    Text("none").foregroundStyle(.secondary)
                }
                ForEach(binding.keys, id: \.display) { key in
                    Text(key.display)
                        .font(.system(.body, design: .monospaced))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(RoundedRectangle(cornerRadius: 4).fill(.quaternary))
                }
                Button("Set") { model.record(mode: mode, action: binding.action, add: false) }
                Button("+") { model.record(mode: mode, action: binding.action, add: true) }
                    .help("Add another key")
                if binding.customized {
                    Button("Reset") { model.reset("keys.\(mode).\(binding.action)") }
                }
            }
        }
    }
}

/// A shell command to copy, with what it's for.
private struct CommandRow: View {
    let label: String
    let command: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
            HStack {
                Text(command)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                    .lineLimit(2)
                Spacer()
                Button(copied ? "Copied" : "Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(command, forType: .string)
                    copied = true
                }
            }
        }
    }
}
