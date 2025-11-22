import SwiftUI
import Combine
import AVFoundation
import AppKit
import UniformTypeIdentifiers
import UserNotifications

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
    @Published var isSelectingRegion = false

    // MARK: Playback
    @Published private(set) var selectedFile: URL?
    @Published var player: AVPlayer?
    @Published var videoSize: CGSize?
    @Published var isPlaying = false
    @Published var currentTime: TimeInterval = 0
    @Published var duration: TimeInterval = 0
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
                size: nil,
                selection: nil,
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
        activeFileID = id
        selectedFile = entry.url
        selection = entry.selection
        metrics = entry.metrics
        progress = entry.progress
        subtitles = entry.subtitles
        errorMessage = entry.errorMessage
        outputURL = entry.outputURL
        duration = entry.duration
        videoSize = entry.size
        lastCueCount = entry.cues
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
            updateFile(id: fileID) { $0.size = size }
        } else {
            videoSize = nil
        }
        let length = asset.duration.seconds
        if length.isFinite {
            duration = length
            updateFile(id: fileID) { $0.duration = length }
        }
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

    private func updateFile(id: UUID, mutate: (inout TrackedFile) -> Void) {
        guard let index = files.firstIndex(where: { $0.id == id }) else { return }
        mutate(&files[index])
    }

    // MARK: - Selection

    func resetSelection() {
        selection = nil
        if let id = activeFileID {
            updateFile(id: id) { $0.selection = nil }
        }
    }

    func startRegionSelection() {
        isSelectingRegion.toggle()
    }

    func finishRegionSelection() {
        isSelectingRegion = false
    }

    func updateSelection(normalized rect: CGRect) {
        selection = rect
        if let id = activeFileID {
            updateFile(id: id) { $0.selection = rect }
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
        _ = ffi.cancel(handle: handle)
        handleToFile.removeValue(forKey: handle)
        lastCuePerHandle.removeValue(forKey: handle)
        updateFile(id: fileID) { file in
            file.status = .canceled
            file.handle = nil
        }
        if let scoped = detectionScopedURLs.removeValue(forKey: handle) {
            scoped.stopAccessingSecurityScopedResource()
        }
    }
    
    func pauseDetection() {
        guard let fileID = activeFileID,
              let handle = files.first(where: { $0.id == fileID })?.handle else { return }
        switch ffi.pause(handle: handle) {
        case .success:
            updateFile(id: fileID) { $0.status = .paused }
        case .failure(let error):
            errorMessage = error.localizedDescription
        }
    }
    
    func resumeDetection() {
        guard let fileID = activeFileID,
              let handle = files.first(where: { $0.id == fileID })?.handle else { return }
        switch ffi.resume(handle: handle) {
        case .success:
            updateFile(id: fileID) { $0.status = .detecting }
        case .failure(let error):
            errorMessage = error.localizedDescription
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
        guard let fileID = activeFileID else { return }
        guard let entry = files.first(where: { $0.id == fileID }) else { return }
        guard let outputURL = entry.outputURL, FileManager.default.fileExists(atPath: outputURL.path) else {
            return
        }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "srt") ?? .plainText]
        panel.canCreateDirectories = true
        panel.nameFieldStringValue = outputURL.lastPathComponent
        panel.begin { response in
            guard response == .OK, let destination = panel.url else { return }
            do {
                if FileManager.default.fileExists(atPath: destination.path) {
                    try FileManager.default.removeItem(at: destination)
                }
                try FileManager.default.copyItem(at: outputURL, to: destination)
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
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
