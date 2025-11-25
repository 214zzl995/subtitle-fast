import SwiftUI
import Combine
import AVFoundation
import AppKit
import UniformTypeIdentifiers
import UserNotifications
import CoreImage

struct SubtitleItem: Identifiable {
    let id = UUID()
    let index: Int
    let timecode: TimeInterval
    let endTime: TimeInterval
    let text: String
    let confidence: Double?
}

struct DetectionMetrics {
    var fps: Double
    var det: Double
    var ocr: Double
    var cues: Int
    var ocrEmpty: Int

    static let empty = DetectionMetrics(fps: 0, det: 0, ocr: 0, cues: 0, ocrEmpty: 0)
}

enum DetectionStatus {
    case idle
    case detecting
    case completed
    case failed(String)
    case canceled
    case paused
}

extension DetectionStatus: Equatable {
    static func == (lhs: DetectionStatus, rhs: DetectionStatus) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle),
             (.detecting, .detecting),
             (.completed, .completed),
             (.canceled, .canceled),
             (.paused, .paused):
            return true
        case (.failed(let lhsMessage), .failed(let rhsMessage)):
            return lhsMessage == rhsMessage
        default:
            return false
        }
    }
}

struct TrackedFile: Identifiable {
    let id: UUID
    let url: URL
    var duration: TimeInterval
    var frameRate: Double?
    var size: CGSize?
    var selection: CGRect?
    var outputURL: URL?
    var subtitles: [SubtitleItem]
    var metrics: DetectionMetrics
    var progress: Double
    var cues: Int
    var status: DetectionStatus
    var errorMessage: String?
    var handle: UInt64?
}

enum PreviewMode: String, CaseIterable, Identifiable, Hashable {
    case color
    case luma
    
    var id: String { rawValue }
}

@MainActor
final class DetectionSession: ObservableObject {
    // MARK: Selection Properties
    @Published var selection: CGRect?
    @Published var threshold: Double = 230
    @Published var tolerance: Double = 20
    @Published var isHighlightActive = false
    @Published var highlightTint: NSColor = .controlAccentColor
    @Published var selectionVisible = true
    @Published var isSamplingThreshold = false

    // MARK: Playback
    @Published private(set) var selectedFile: URL?
    @Published var player: AVPlayer?
    @Published var videoSize: CGSize?
    @Published var isPlaying = false
    @Published var currentTime: TimeInterval = 0
    @Published var duration: TimeInterval = 0
    @Published var videoFrameRate: Double?
    @Published var previewMode: PreviewMode = .color
    
    // File list
    @Published var files: [TrackedFile] = []
    @Published var activeFileID: UUID?

    // MARK: Detection
    @Published var progress: Double = 0
    @Published var errorMessage: String?
    @Published var metrics = DetectionMetrics.empty
    @Published var subtitles: [SubtitleItem] = []

    var activeFile: TrackedFile? {
        files.first(where: { $0.id == activeFileID })
    }
    var activeStatus: DetectionStatus {
        activeFile?.status ?? .idle
    }
    var isActiveDetecting: Bool {
        if case .detecting = activeStatus { return true }
        if case .paused = activeStatus { return true }
        return false
    }

    private var timeObserver: Any?
    private var scopedURL: URL?
    private var outputURL: URL?
    private var currentAsset: AVAsset?
    private var lastCuePerHandle: [UInt64: Int] = [:]
    private var lastCueCount: Int = 0
    private var handleToFile: [UInt64: UUID] = [:]
    private var detectionScopedURLs: [UInt64: URL] = [:]
    private let samplesPerSecond: UInt32 = 7
    private let ffi = SubtitleFastFFI.shared
    private let ciContext = CIContext()
    private let defaultSelectionNormalized = CGRect(x: 0.15, y: 0.75, width: 0.7, height: 0.25)
    private var resumePlaybackAfterSampling = false

    init() {
        ffi.registerCallbacks(
            onProgress: { [weak self] payload in
                DispatchQueue.main.async {
                    self?.handleProgress(payload)
                }
            },
            onError: { [weak self] message in
                DispatchQueue.main.async {
                    guard let self else { return }
                    self.errorMessage = message
                    if let handleID = self.handleToFile.first?.key, let fileID = self.handleToFile[handleID] {
                        self.updateFile(id: fileID) { file in
                            file.status = .failed(message)
                            file.errorMessage = message
                        }
                        self.notifyCompletion(for: fileID, success: false)
                    }
                }
            }
        )
    }
    // MARK: - File + Playback

    func pickFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.movie, .mpeg4Movie, .quickTimeMovie]
        panel.begin { [weak self] response in
            guard response == .OK else { return }
            self?.load(from: panel.urls)
        }
    }

    func load(from url: URL) {
        load(from: [url])
    }

    func load(from urls: [URL]) {
        guard !urls.isEmpty else { return }
        var lastID: UUID?
        for url in urls {
            if let existing = files.first(where: { $0.url == url }) {
                lastID = existing.id
                continue
            }
            let entry = TrackedFile(
                id: UUID(),
                url: url,
                duration: 0,
                frameRate: nil,
                size: nil,
                selection: clampNormalized(defaultSelectionNormalized),
                outputURL: nil,
                subtitles: [],
                metrics: .empty,
                progress: 0,
                cues: 0,
                status: .idle,
                errorMessage: nil
            )
            files.append(entry)
            lastID = entry.id
        }
        if let id = lastID {
            activateFile(id: id)
        }
    }

    func activateFile(id: UUID) {
        guard let entry = files.first(where: { $0.id == id }) else { return }
        isSamplingThreshold = false
        resumePlaybackAfterSampling = false
        activeFileID = id
        selectedFile = entry.url
        selection = clampNormalized(entry.selection ?? defaultSelectionNormalized)
        metrics = entry.metrics
        progress = entry.progress
        subtitles = entry.subtitles
        errorMessage = entry.errorMessage
        outputURL = entry.outputURL
        duration = entry.duration
        videoFrameRate = entry.frameRate
        videoSize = entry.size
        lastCueCount = entry.cues
        ensureSelection(for: id)
        configurePlayer(for: entry)
    }

    private func configurePlayer(for entry: TrackedFile) {
        let url = entry.url
        if let observer = timeObserver, let player {
            player.removeTimeObserver(observer)
            timeObserver = nil
        }
        if let scoped = scopedURL {
            scoped.stopAccessingSecurityScopedResource()
        }
        if url.startAccessingSecurityScopedResource() {
            scopedURL = url
        } else {
            scopedURL = nil
        }

        let asset = AVAsset(url: url)
        currentAsset = asset
        updateVideoMetadata(for: asset, fileID: entry.id)

        let item = makePlayerItem(for: asset)
        let newPlayer = AVPlayer()
        newPlayer.replaceCurrentItem(with: item)
        newPlayer.actionAtItemEnd = .pause
        player = newPlayer

        setupTimeObserver()
        newPlayer.seek(to: .zero, toleranceBefore: .zero, toleranceAfter: .zero)
        newPlayer.pause()
        isPlaying = false
        currentTime = 0
    }

    private func makePlayerItem(for asset: AVAsset) -> AVPlayerItem {
        let item = AVPlayerItem(asset: asset)
        if previewMode == .luma {
            item.videoComposition = AVVideoComposition(asset: asset) { request in
                let image = request.sourceImage.clampedToExtent()
                let gray = image.applyingFilter(
                    "CIColorControls",
                    parameters: [kCIInputSaturationKey: 0.0]
                )
                request.finish(with: gray, context: nil)
            }
        } else {
            item.videoComposition = nil
        }
        return item
    }

    private func setupTimeObserver() {
        if let observer = timeObserver, let player {
            player.removeTimeObserver(observer)
        }
        guard let player else { return }
        let interval = CMTime(seconds: 0.2, preferredTimescale: 600)
        timeObserver = player.addPeriodicTimeObserver(forInterval: interval, queue: .main) { [weak self] time in
            guard let self else { return }
            Task { @MainActor in
                self.currentTime = time.seconds
                self.isPlaying = self.player?.timeControlStatus == .playing
                if self.duration == 0 {
                    self.duration = self.player?.currentItem?.asset.duration.seconds ?? 0
                    if let id = self.activeFileID {
                        self.updateFile(id: id) { $0.duration = self.duration }
                    }
                }
            }
        }
    }

    private func updateVideoMetadata(for asset: AVAsset, fileID: UUID) {
        if let track = asset.tracks(withMediaType: .video).first {
            let transformed = track.naturalSize.applying(track.preferredTransform)
            let size = CGSize(width: abs(transformed.width), height: abs(transformed.height))
            videoSize = size
            let fps = resolvedFrameRate(from: track)
            videoFrameRate = fps
            updateFile(id: fileID) {
                $0.size = size
                $0.frameRate = fps
            }
        } else {
            videoSize = nil
            videoFrameRate = nil
            updateFile(id: fileID) {
                $0.size = nil
                $0.frameRate = nil
            }
        }
        let length = asset.duration.seconds
        if length.isFinite {
            duration = length
            updateFile(id: fileID) { $0.duration = length }
        } else {
            duration = 0
        }
    }

    private func resolvedFrameRate(from track: AVAssetTrack) -> Double? {
        if track.nominalFrameRate > 0 {
            return Double(track.nominalFrameRate)
        }
        let minDuration = track.minFrameDuration
        if minDuration.isNumeric && minDuration.value != 0 {
            return Double(minDuration.timescale) / Double(minDuration.value)
        }
        return nil
    }

    func togglePlayPause() {
        guard let player else { return }
        if player.timeControlStatus == .playing {
            player.pause()
            isPlaying = false
        } else {
            player.play()
            isPlaying = true
        }
    }

    func seek(to time: TimeInterval) {
        guard let player else { return }
        let cmTime = CMTime(seconds: time, preferredTimescale: 600)
        player.seek(to: cmTime, toleranceBefore: .zero, toleranceAfter: .zero)
        currentTime = time
    }

    func applyPreviewMode() {
        guard let asset = currentAsset, let player else { return }
        let wasPlaying = player.timeControlStatus == .playing
        let currentPosition = player.currentTime()
        let item = makePlayerItem(for: asset)
        player.replaceCurrentItem(with: item)
        player.seek(to: currentPosition, toleranceBefore: .zero, toleranceAfter: .zero)
        if wasPlaying {
            player.play()
        }
    }

    func stepFrame(forward: Bool) {
        guard let player else { return }
        player.pause()
        isPlaying = false

        if let item = player.currentItem {
            let stepCount = forward ? 1 : -1
            item.step(byCount: stepCount)
            currentTime = player.currentTime().seconds
            return
        }

        guard let fps = videoFrameRate, fps > 0 else { return }
        let delta = 1.0 / fps
        let target = forward ? currentTime + delta : currentTime - delta
        seek(to: target)
    }

    func jumpBy(seconds: Double) {
        guard let player else { return }
        let maxDuration = duration > 0 ? duration : player.currentItem?.duration.seconds ?? 0
        let effectiveDuration = maxDuration.isFinite ? maxDuration : 0
        let clampedUpperBound = effectiveDuration > 0 ? effectiveDuration : currentTime + seconds
        let target = max(0, min(clampedUpperBound, currentTime + seconds))
        seek(to: target)
    }

    func snapshotCurrentFrame(lumaOnly: Bool) -> CGImage? {
        guard let asset = currentAsset else { return nil }
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.requestedTimeToleranceAfter = .zero
        generator.requestedTimeToleranceBefore = .zero
        let time = player?.currentTime() ?? .zero
        do {
            let image = try generator.copyCGImage(at: time, actualTime: nil)
            guard lumaOnly else { return image }
            let ciImage = CIImage(cgImage: image).applyingFilter(
                "CIColorControls",
                parameters: [kCIInputSaturationKey: 0.0]
            )
            return ciContext.createCGImage(ciImage, from: ciImage.extent) ?? image
        } catch {
            return nil
        }
    }

    private func updateFile(id: UUID, mutate: (inout TrackedFile) -> Void) {
        guard let index = files.firstIndex(where: { $0.id == id }) else { return }
        mutate(&files[index])
    }

    private func ensureSelection(for id: UUID) {
        let rect = clampNormalized(defaultSelectionNormalized)
        guard let index = files.firstIndex(where: { $0.id == id }) else { return }
        if files[index].selection == nil {
            files[index].selection = rect
        }
        if selection == nil || activeFileID == id {
            selection = files[index].selection ?? rect
        }
    }

    private func clampNormalized(_ rect: CGRect) -> CGRect {
        CGRect(
            x: min(max(rect.origin.x, 0), 1),
            y: min(max(rect.origin.y, 0), 1),
            width: min(max(rect.width, 0), 1 - min(max(rect.origin.x, 0), 1)),
            height: min(max(rect.height, 0), 1 - min(max(rect.origin.y, 0), 1))
        )
    }

    // MARK: - Selection

    func beginThresholdSampling() {
        guard selectedFile != nil else { return }
        if isSamplingThreshold { return }
        resumePlaybackAfterSampling = isPlaying
        if isPlaying {
            player?.pause()
            isPlaying = false
        }
        isSamplingThreshold = true
    }

    func applySampledThreshold(_ value: Double) {
        let clamped = min(255, max(0, value.rounded()))
        threshold = clamped
        finishThresholdSampling()
    }

    func cancelThresholdSampling() {
        finishThresholdSampling()
    }

    private func finishThresholdSampling() {
        guard isSamplingThreshold else { return }
        isSamplingThreshold = false
        if resumePlaybackAfterSampling, let player {
            player.play()
            isPlaying = true
        }
        resumePlaybackAfterSampling = false
    }

    func resetSelection() {
        let rect = clampNormalized(defaultSelectionNormalized)
        selection = rect
        selectionVisible = true
        if let id = activeFileID {
            updateFile(id: id) { $0.selection = rect }
        }
    }

    func updateSelection(normalized rect: CGRect) {
        let clamped = clampNormalized(rect)
        selection = clamped
        if let id = activeFileID {
            updateFile(id: id) { $0.selection = clamped }
        }
    }

    // MARK: - Detection

    func startDetection() {
        guard let fileID = activeFileID, let file = files.first(where: { $0.id == fileID }) else {
            errorMessage = NSLocalizedString("ui.error_no_file", comment: "no file")
            return
        }
        let roi = file.selection ?? selection
        guard let region = roi else {
            errorMessage = NSLocalizedString("ui.error_no_selection", comment: "no selection")
            return
        }

        errorMessage = nil
        subtitles = []
        metrics = .empty
        progress = 0
        lastCueCount = 0

        let output = makeOutputURL(for: file.url, id: fileID)
        outputURL = output
        try? FileManager.default.removeItem(at: output)
        updateFile(id: fileID) { entry in
            entry.status = .detecting
            entry.progress = 0
            entry.metrics = .empty
            entry.subtitles = []
            entry.outputURL = output
            entry.errorMessage = nil
            entry.cues = 0
            entry.handle = nil
            self.lastCueCount = 0
        }

        if let scoped = scopedURL, scoped != file.url {
            scoped.stopAccessingSecurityScopedResource()
        }
        var pendingScopedURL: URL?
        if file.url.startAccessingSecurityScopedResource() {
            pendingScopedURL = file.url
        }

        let target = UInt8(clamping: Int(threshold))
        let delta = UInt8(clamping: Int(tolerance))

        let result = ffi.startRun(
            input: file.url,
            output: output,
            decoderBackend: nil,
            samplesPerSecond: samplesPerSecond,
            detectorTarget: target,
            detectorDelta: delta,
            roi: region
        )

        switch result {
        case .success(let handle):
            handleToFile[handle] = fileID
            player?.pause()
            isPlaying = false
            updateFile(id: fileID) { entry in
                entry.handle = handle
                entry.status = .detecting
            }
            lastCuePerHandle[handle] = 0
            if let url = pendingScopedURL {
                detectionScopedURLs[handle] = url
            }
        case .failure(let error):
            errorMessage = error.localizedDescription
            updateFile(id: fileID) { entry in
                entry.status = .failed(error.localizedDescription)
                entry.errorMessage = error.localizedDescription
            }
            if let url = pendingScopedURL {
                url.stopAccessingSecurityScopedResource()
            }
        }
    }

    func cancelDetection() {
        guard let fileID = activeFileID,
              let handle = files.first(where: { $0.id == fileID })?.handle else { return }
        cancelHandle(handle)
        metrics = .empty
        progress = 0
        subtitles = []
        errorMessage = nil
        lastCueCount = 0
        updateFile(id: fileID) { file in
            file.status = .canceled
            file.handle = nil
            file.metrics = .empty
            file.progress = 0
            file.subtitles = []
            file.cues = 0
            file.errorMessage = nil
        }
    }
    
    func pauseDetection() {
        guard let fileID = activeFileID else { return }
        pauseFile(id: fileID)
    }
    
    func resumeDetection() {
        guard let fileID = activeFileID else { return }
        resumeFile(id: fileID)
    }

    func removeFile(id: UUID) {
        guard let index = files.firstIndex(where: { $0.id == id }) else { return }
        let file = files[index]
        if case .detecting = file.status {
            return
        }

        if let handle = file.handle {
            cancelHandle(handle)
        }

        let wasActive = activeFileID == file.id
        files.remove(at: index)

        if wasActive {
            clearActiveState()
            if let next = files.first {
                activateFile(id: next.id)
            }
        }
    }

    func pauseFile(id: UUID) {
        guard let file = files.first(where: { $0.id == id }), let handle = file.handle else { return }
        switch ffi.pause(handle: handle) {
        case .success:
            updateFile(id: id) { $0.status = .paused }
            if activeFileID == id {
                // Keep progress visible but flag paused state.
                errorMessage = nil
            }
        case .failure(let error):
            if activeFileID == id {
                errorMessage = error.localizedDescription
            }
        }
    }

    func resumeFile(id: UUID) {
        guard let file = files.first(where: { $0.id == id }), let handle = file.handle else { return }
        switch ffi.resume(handle: handle) {
        case .success:
            updateFile(id: id) { $0.status = .detecting }
            if activeFileID == id {
                errorMessage = nil
            }
        case .failure(let error):
            if activeFileID == id {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func handleProgress(_ payload: GuiProgressPayload) {
        let update = payload.update
        guard let fileID = handleToFile[update.handle_id] else { return }

        let lastCues = lastCuePerHandle[update.handle_id] ?? 0
        let newCueCount = Int(update.cues)

        let snapshot = DetectionMetrics(
            fps: update.fps,
            det: update.det_ms,
            ocr: update.ocr_ms,
            cues: Int(update.cues),
            ocrEmpty: Int(update.ocr_empty)
        )
        let newProgress = max(0, min(1, update.progress))
        updateFile(id: fileID) { entry in
            entry.metrics = snapshot
            entry.progress = newProgress
            entry.cues = Int(update.cues)
            if entry.status != .paused {
                entry.status = .detecting
            }
        }
        if let activeID = activeFileID, activeID == fileID {
            metrics = snapshot
            progress = newProgress
        }

        if newCueCount > lastCues {
            if let text = payload.subtitleText, !text.isEmpty {
                let item = SubtitleItem(
                    index: newCueCount,
                    timecode: update.subtitle_start_ms / 1000.0,
                    endTime: update.subtitle_end_ms / 1000.0,
                    text: text,
                    confidence: nil
                )
                appendSubtitle(item, for: fileID)
            }
        }

        lastCuePerHandle[update.handle_id] = max(lastCues, newCueCount)

        if update.completed {
            updateFile(id: fileID) { entry in
                entry.status = .completed
                entry.progress = 1.0
                entry.handle = nil
            }
            loadSubtitlesFromOutput(for: fileID)
            notifyCompletion(for: fileID, success: true)
            handleToFile.removeValue(forKey: update.handle_id)
            lastCuePerHandle.removeValue(forKey: update.handle_id)
            if let scoped = detectionScopedURLs.removeValue(forKey: update.handle_id) {
                scoped.stopAccessingSecurityScopedResource()
            }
            if let activeID = activeFileID, activeID == fileID {
                metrics = snapshot
                progress = newProgress
            }
        }
    }

    private func appendSubtitle(_ item: SubtitleItem, for fileID: UUID) {
        updateFile(id: fileID) { entry in
            entry.subtitles.append(item)
            entry.cues = entry.subtitles.count
        }
        if activeFileID == fileID {
            subtitles.append(item)
        }
    }

    private func loadSubtitlesFromOutput(for fileID: UUID) {
        guard let entry = files.first(where: { $0.id == fileID }) else { return }
        let output = entry.outputURL ?? outputURL
        guard let output, FileManager.default.fileExists(atPath: output.path) else {
            return
        }
        guard let contents = try? String(contentsOf: output, encoding: .utf8) else {
            return
        }
        let parsed = SubtitleParser.parse(srt: contents)
        updateFile(id: fileID) { entry in
            entry.subtitles = parsed
            entry.cues = parsed.count
            entry.outputURL = output
        }
        if activeFileID == fileID {
            subtitles = parsed
        }
    }

    func exportSubtitles() {

        guard let fileID = activeFileID,
              let entry = files.first(where: { $0.id == fileID }) else { return }

        let outputURL = entry.outputURL
        let hasOutputFile = outputURL.map { FileManager.default.fileExists(atPath: $0.path) } ?? false
        let hasSubtitles = !entry.subtitles.isEmpty

        guard hasOutputFile || hasSubtitles else {
            errorMessage = NSLocalizedString("ui.error_no_subtitles_export", comment: "no subtitles to export")
            return
        }

        let defaultName: String = {
            let base = entry.url.deletingPathExtension().lastPathComponent
            return "\(base).srt"
        }()

        let serialized = hasSubtitles && !hasOutputFile ? serializeSubtitles(entry.subtitles) : nil
        let subtitlesSnapshot = entry.subtitles
        errorMessage = nil
        NSApplication.shared.activate(ignoringOtherApps: true)
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "srt") ?? .plainText]
        panel.allowedFileTypes = ["srt"]
        panel.canCreateDirectories = true
        panel.nameFieldStringValue = defaultName

        let completion: (URL) -> Void = { [weak self] destination in
            guard let self else { return }
            do {
                if FileManager.default.fileExists(atPath: destination.path) {
                    try FileManager.default.removeItem(at: destination)
                }
                if hasOutputFile, let output = outputURL {
                    try FileManager.default.copyItem(at: output, to: destination)
                } else {
                    let srtText = serialized ?? self.serializeSubtitles(subtitlesSnapshot)
                    if srtText.isEmpty {
                        throw NSError(domain: "subtitle-fast.export", code: -1, userInfo: [
                            NSLocalizedDescriptionKey: NSLocalizedString("ui.error_no_subtitles_export", comment: "")
                        ])
                    }
                    try srtText.write(to: destination, atomically: true, encoding: .utf8)
                }
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }

        if let window = NSApplication.shared.keyWindow ?? NSApplication.shared.mainWindow {
            panel.beginSheetModal(for: window) { response in
                guard response == .OK, let url = panel.url else { return }
                completion(url)
            }
        } else {
            let response = panel.runModal()
            guard response == .OK, let url = panel.url else { return }
            completion(url)
        }
    }

    private func serializeSubtitles(_ items: [SubtitleItem]) -> String {
        items
            .map { item -> String in
                let start = formatSRTTime(item.timecode)
                let end = formatSRTTime(item.endTime)
                return "\(item.index)\n\(start) --> \(end)\n\(item.text)"
            }
            .joined(separator: "\n\n")
            .appending(items.isEmpty ? "" : "\n")
    }

    private func formatSRTTime(_ time: TimeInterval) -> String {
        let totalMillis = Int((time * 1000).rounded())
        let hours = totalMillis / 3_600_000
        let minutes = (totalMillis % 3_600_000) / 60_000
        let seconds = (totalMillis % 60_000) / 1000
        let millis = totalMillis % 1000
        return String(format: "%02d:%02d:%02d,%03d", hours, minutes, seconds, millis)
    }

    private func makeOutputURL(for input: URL, id: UUID) -> URL {
        let name = input.deletingPathExtension().lastPathComponent
        return FileManager.default.temporaryDirectory.appendingPathComponent("\(name)-\(id.uuidString.prefix(8))-subtitle-fast.srt")
    }

    private func notifyCompletion(for fileID: UUID, success: Bool) {
        guard let entry = files.first(where: { $0.id == fileID }) else { return }
        NotificationManager.shared.notifyDetectionFinished(
            fileName: entry.url.lastPathComponent,
            success: success,
            message: success ? nil : errorMessage
        )
    }

    private func cancelHandle(_ handle: UInt64) {
        _ = ffi.cancel(handle: handle)
        handleToFile.removeValue(forKey: handle)
        lastCuePerHandle.removeValue(forKey: handle)
        if let scoped = detectionScopedURLs.removeValue(forKey: handle) {
            scoped.stopAccessingSecurityScopedResource()
        }
    }

    private func clearActiveState() {
        if let observer = timeObserver, let player {
            player.removeTimeObserver(observer)
            timeObserver = nil
        }
        player?.pause()
        player = nil
        isPlaying = false
        selectedFile = nil
        selection = nil
        metrics = .empty
        progress = 0
        currentTime = 0
        subtitles = []
        errorMessage = nil
        duration = 0
        videoFrameRate = nil
        videoSize = nil
        outputURL = nil
        currentAsset = nil
        lastCueCount = 0
        activeFileID = nil
        if let scoped = scopedURL {
            scoped.stopAccessingSecurityScopedResource()
            scopedURL = nil
        }
    }
}

enum SubtitleParser {
    static func parse(srt: String) -> [SubtitleItem] {
        let normalized = srt.replacingOccurrences(of: "\r\n", with: "\n")
        let blocks = normalized.components(separatedBy: "\n\n")
        var items: [SubtitleItem] = []

        for (blockIndex, block) in blocks.enumerated() {
            let lines = block.split(whereSeparator: \.isNewline)
            guard !lines.isEmpty else { continue }

            let (timeLine, textStart) = timeLineAndStartIndex(lines: lines)
            guard let timeLine else { continue }

            let parts = timeLine.split(separator: " ")
            guard parts.count >= 3 else { continue }

            guard
                let start = clockTime(String(parts[0])),
                let end = clockTime(String(parts[2]))
            else { continue }

            let text = lines[textStart...].joined(separator: "\n")
            let number = Int(lines.first ?? "") ?? (blockIndex + 1)

            items.append(
                SubtitleItem(
                    index: number,
                    timecode: start,
                    endTime: end,
                    text: text.trimmingCharacters(in: .whitespacesAndNewlines),
                    confidence: nil
                )
            )
        }

        return items
    }

    private static func timeLineAndStartIndex(lines: [Substring]) -> (Substring?, Int) {
        if lines.count >= 2, lines[1].contains("-->") {
            return (lines[1], 2)
        }
        if lines.first?.contains("-->") == true {
            return (lines.first, 1)
        }
        return (nil, 0)
    }

    private static func clockTime(_ raw: String) -> TimeInterval? {
        let parts = raw.split(separator: ":")
        guard parts.count == 3 else { return nil }
        let secParts = parts[2].split(separator: ",")
        guard secParts.count == 2 else { return nil }
        guard
            let hours = Int(parts[0]),
            let minutes = Int(parts[1]),
            let seconds = Int(secParts[0]),
            let millis = Int(secParts[1])
        else { return nil }

        let base = hours * 3600 + minutes * 60 + seconds
        return Double(base) + Double(millis) / 1000.0
    }
}
