import CoreGraphics
import Foundation

// MARK: - C FFI Structures

/// Configuration for starting a subtitle detection run
struct GuiRunConfig {
    var input_path: UnsafePointer<CChar>?
    var output_path: UnsafePointer<CChar>?
    var decoder_backend: UnsafePointer<CChar>?
    var detection_samples_per_second: UInt32
    var detector_target: UInt8
    var detector_delta: UInt8
    var roi_x: Float
    var roi_y: Float
    var roi_width: Float
    var roi_height: Float
    var roi_enabled: UInt8
}

/// Result from starting a detection run
struct GuiRunResult {
    var handle_id: UInt64
    var error_code: Int32
}

/// Progress update from the detection process
struct GuiProgressUpdate {
    var samples_seen: UInt64
    var latest_frame_index: UInt64
    var total_frames: UInt64
    var fps: Double
    var det_ms: Double
    var seg_ms: Double
    var pf_ms: Double
    var ocr_ms: Double
    var writer_ms: Double
    var cues: UInt64
    var ocr_empty: UInt64
    var progress: Double
    var completed: Bool
}

/// Error message from the detection process
struct GuiProgressError {
    var message: UnsafePointer<CChar>?
}

// MARK: - C Callbacks

typealias ProgressCallback = @convention(c) (UnsafeRawPointer?, UnsafeMutableRawPointer?) -> Void
typealias ErrorCallback = @convention(c) (UnsafeRawPointer?, UnsafeMutableRawPointer?) -> Void

struct GuiProgressCallbacks {
    var user_data: UnsafeMutableRawPointer?
    var on_progress: ProgressCallback?
    var on_error: ErrorCallback?
}

// MARK: - C Function Declarations

@_silgen_name("progress_gui_init")
func progress_gui_init(_ callbacks: UnsafePointer<GuiProgressCallbacks>)

@_silgen_name("progress_gui_shutdown")
func progress_gui_shutdown()

@_silgen_name("subtitle_fast_gui_start")
func subtitle_fast_gui_start(_ config: UnsafePointer<GuiRunConfig>?) -> GuiRunResult

@_silgen_name("subtitle_fast_gui_cancel")
func subtitle_fast_gui_cancel(_ handle: UInt64) -> Int32

// MARK: - FFI Bridge

/// Swift bridge to the Rust subtitle-fast library
final class SubtitleFastFFI {
    static let shared = SubtitleFastFFI()
    
    private var callbacksRegistered = false
    private var progressHandler: ((GuiProgressUpdate) -> Void)?
    private var errorHandler: ((String) -> Void)?
    
    private init() {}
    
    deinit {
        if callbacksRegistered {
            progress_gui_shutdown()
        }
    }
    
    // MARK: - Public API
    
    /// Register callbacks for progress and error handling
    func registerCallbacks(
        onProgress: @escaping (GuiProgressUpdate) -> Void,
        onError: @escaping (String) -> Void
    ) {
        progressHandler = onProgress
        errorHandler = onError
        
        guard !callbacksRegistered else { return }
        
        var callbacks = GuiProgressCallbacks(
            user_data: nil,
            on_progress: progressTrampoline,
            on_error: errorTrampoline
        )
        
        withUnsafePointer(to: &callbacks) {
            progress_gui_init($0)
        }
        callbacksRegistered = true
    }
    
    /// Start a subtitle detection run
    func startRun(
        input: URL,
        output: URL?,
        decoderBackend: String?,
        samplesPerSecond: UInt32,
        detectorTarget: UInt8,
        detectorDelta: UInt8,
        roi: CGRect?
    ) -> Result<UInt64, Error> {
        var config = GuiRunConfig(
            input_path: nil,
            output_path: nil,
            decoder_backend: nil,
            detection_samples_per_second: samplesPerSecond,
            detector_target: detectorTarget,
            detector_delta: detectorDelta,
            roi_x: 0,
            roi_y: 0,
            roi_width: 0,
            roi_height: 0,
            roi_enabled: 0
        )
        
        if let roi {
            let clampedX = max(0, min(roi.origin.x, 1))
            let clampedY = max(0, min(roi.origin.y, 1))
            let clampedWidth = max(0, min(roi.size.width, 1))
            let clampedHeight = max(0, min(roi.size.height, 1))
            config.roi_x = Float(clampedX)
            config.roi_y = Float(clampedY)
            config.roi_width = Float(clampedWidth)
            config.roi_height = Float(clampedHeight)
            config.roi_enabled = 1
        }
        
        let result: GuiRunResult = input.path.withCString { inputPtr in
            config.input_path = inputPtr
            
            if let output = output {
                return output.path.withCString { outputPtr in
                    config.output_path = outputPtr
                    return invokeStart(config: &config, decoderBackend: decoderBackend)
                }
            } else {
                return invokeStart(config: &config, decoderBackend: decoderBackend)
            }
        }
        
        if result.error_code == 0 && result.handle_id != 0 {
            return .success(result.handle_id)
        } else {
            return .failure(FFIError.startFailed(code: result.error_code))
        }
    }
    
    /// Cancel a running detection
    func cancel(handle: UInt64) -> Result<Void, Error> {
        let code = subtitle_fast_gui_cancel(handle)
        if code == 0 {
            return .success(())
        } else {
            return .failure(FFIError.cancelFailed(code: code))
        }
    }
    
    // MARK: - Private Helpers
    
    private func invokeStart(
        config: inout GuiRunConfig,
        decoderBackend: String?
    ) -> GuiRunResult {
        if let backend = decoderBackend {
            return backend.withCString { backendPtr in
                config.decoder_backend = backendPtr
                return withUnsafePointer(to: &config) { subtitle_fast_gui_start($0) }
            }
        } else {
            return withUnsafePointer(to: &config) { subtitle_fast_gui_start($0) }
        }
    }
    
    fileprivate func handleProgress(_ update: GuiProgressUpdate) {
        progressHandler?(update)
    }
    
    fileprivate func handleError(_ message: String) {
        errorHandler?(message)
    }
}

enum FFIError: Error, LocalizedError {
    case startFailed(code: Int32)
    case cancelFailed(code: Int32)
    
    var errorDescription: String? {
        switch self {
        case .startFailed(let code):
            return "Failed to start detection (Error code: \(code))"
        case .cancelFailed(let code):
            return "Failed to cancel detection (Error code: \(code))"
        }
    }
}

// MARK: - C Callback Trampolines

private let progressTrampoline: ProgressCallback = { updatePtr, _ in
    guard let updatePtr = updatePtr else { return }
    let update = updatePtr.assumingMemoryBound(to: GuiProgressUpdate.self).pointee
    SubtitleFastFFI.shared.handleProgress(update)
}

private let errorTrampoline: ErrorCallback = { errorPtr, _ in
    guard let errorPtr = errorPtr else { return }
    let error = errorPtr.assumingMemoryBound(to: GuiProgressError.self).pointee
    guard let cString = error.message else { return }
    let message = String(cString: cString)
    SubtitleFastFFI.shared.handleError(message)
}
