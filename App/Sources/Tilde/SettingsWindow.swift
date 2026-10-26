import AppKit
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
    case general, preview, editing, search, keys
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
            Section("zoxide") {
                Toggle("Rank folders by zoxide's frecency", isOn: model.bool("zoxide.enabled") { $0.zoxideEnabled })
                Toggle("Tell zoxide about folders opened in Tilde",
                       isOn: model.bool("zoxide.add_on_visit") { $0.zoxideAddOnVisit })
            }
        }
        .formStyle(.grouped)
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
