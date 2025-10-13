#ifdef _WIN32

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfobjects.h>
#include <mfreadwrite.h>
#include <mferror.h>
#include <propvarutil.h>
#include <combaseapi.h>

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <utility>

namespace {

template <typename T>
void safe_release(T** value) {
    if (value && *value) {
        (*value)->Release();
        *value = nullptr;
    }
}

std::string format_hresult(const char* label, HRESULT hr) {
    char buffer[128];
    std::snprintf(
        buffer,
        sizeof(buffer),
        "%s failed: 0x%08lx",
        label,
        static_cast<unsigned long>(hr)
    );
    return std::string(buffer);
}

struct ScopedCoInitialize {
    HRESULT result;

    ScopedCoInitialize() : result(CoInitializeEx(nullptr, COINIT_MULTITHREADED)) {}

    ~ScopedCoInitialize() {
        if (SUCCEEDED(result)) {
            CoUninitialize();
        }
    }

    bool ok() const {
        return SUCCEEDED(result) || result == RPC_E_CHANGED_MODE;
    }

    std::string error_message() const {
        return format_hresult("CoInitializeEx", result);
    }
};

struct ScopedMediaFoundation {
    HRESULT result;

    ScopedMediaFoundation() : result(MFStartup(MF_VERSION, MFSTARTUP_FULL)) {}

    ~ScopedMediaFoundation() {
        if (SUCCEEDED(result)) {
            MFShutdown();
        }
    }

    bool ok() const {
        return SUCCEEDED(result);
    }

    std::string error_message() const {
        return format_hresult("MFStartup", result);
    }
};

std::wstring utf8_to_wide(const char* utf8, std::string& error) {
    if (!utf8) {
        error = "input path is null";
        return std::wstring();
    }
    int required = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, nullptr, 0);
    if (required <= 0) {
        error = "failed to convert UTF-8 path to UTF-16";
        return std::wstring();
    }
    std::wstring wide;
    wide.resize(static_cast<size_t>(required - 1));
    if (required > 1) {
        int written = MultiByteToWideChar(
            CP_UTF8,
            0,
            utf8,
            -1,
            wide.data(),
            required
        );
        if (written != required) {
            error = "failed to convert UTF-8 path to UTF-16";
            return std::wstring();
        }
    }
    return wide;
}

char* duplicate_string(const std::string& value) {
    size_t size = value.size() + 1;
    char* buffer = static_cast<char*>(CoTaskMemAlloc(size));
    if (!buffer) {
        return nullptr;
    }
    std::memcpy(buffer, value.c_str(), size);
    return buffer;
}

void set_error(char** out, const std::string& message) {
    if (!out) {
        return;
    }
    *out = duplicate_string(message);
}

struct FrameLock {
    IMFMediaBuffer* buffer = nullptr;
    IMF2DBuffer* buffer2d = nullptr;
    BYTE* data = nullptr;
    DWORD contiguous_length = 0;
    LONG stride = 0;
    bool locked2d = false;
    bool locked_raw = false;

    ~FrameLock() {
        unlock();
    }

    HRESULT lock(IMFMediaBuffer* source_buffer, UINT32 width) {
        buffer = source_buffer;
        if (!buffer) {
            return E_POINTER;
        }
        buffer->AddRef();
        HRESULT hr = buffer->QueryInterface(
            __uuidof(IMF2DBuffer),
            reinterpret_cast<void**>(&buffer2d)
        );
        if (SUCCEEDED(hr) && buffer2d) {
            hr = buffer2d->Lock2D(&data, &stride);
            if (SUCCEEDED(hr)) {
                locked2d = true;
                contiguous_length = 0;
                return S_OK;
            }
            safe_release(&buffer2d);
        }
        BYTE* raw = nullptr;
        DWORD max_length = 0;
        DWORD current_length = 0;
        hr = buffer->Lock(&raw, &max_length, &current_length);
        if (FAILED(hr)) {
            return hr;
        }
        data = raw;
        contiguous_length = current_length;
        stride = static_cast<LONG>(width);
        locked_raw = true;
        return S_OK;
    }

    void unlock() {
        if (locked2d && buffer2d) {
            buffer2d->Unlock2D();
        } else if (locked_raw && buffer) {
            buffer->Unlock();
        }
        safe_release(&buffer2d);
        if (buffer) {
            buffer->Release();
            buffer = nullptr;
        }
        data = nullptr;
        contiguous_length = 0;
        stride = 0;
        locked2d = false;
        locked_raw = false;
    }
};

} // namespace

extern "C" {

struct CMftProbeResult {
    bool has_value;
    uint64_t value;
    char* error;
};

struct CMftFrame {
    const uint8_t* data;
    size_t data_len;
    uint32_t width;
    uint32_t height;
    size_t stride;
    double timestamp_seconds;
    uint64_t frame_index;
};

typedef bool(__cdecl* CMftFrameCallback)(const CMftFrame*, void*);

bool mft_probe_total_frames(const char* path, CMftProbeResult* result) {
    if (!result) {
        return false;
    }
    result->has_value = false;
    result->value = 0;
    result->error = nullptr;

    std::string conversion_error;
    std::wstring wide_path = utf8_to_wide(path, conversion_error);
    if (!conversion_error.empty()) {
        set_error(&result->error, conversion_error);
        return false;
    }

    ScopedCoInitialize coinitialize;
    if (!coinitialize.ok()) {
        set_error(&result->error, coinitialize.error_message());
        return false;
    }

    ScopedMediaFoundation media_foundation;
    if (!media_foundation.ok()) {
        set_error(&result->error, media_foundation.error_message());
        return false;
    }

    IMFAttributes* attributes = nullptr;
    HRESULT hr = MFCreateAttributes(&attributes, 1);
    if (FAILED(hr)) {
        set_error(&result->error, format_hresult("MFCreateAttributes", hr));
        safe_release(&attributes);
        return false;
    }
    attributes->SetUINT32(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, TRUE);

    IMFSourceReader* reader = nullptr;
    hr = MFCreateSourceReaderFromURL(wide_path.c_str(), attributes, &reader);
    safe_release(&attributes);
    if (FAILED(hr)) {
        set_error(&result->error, format_hresult("MFCreateSourceReaderFromURL", hr));
        safe_release(&reader);
        return false;
    }

    hr = reader->SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS, FALSE);
    if (FAILED(hr)) {
        set_error(&result->error, format_hresult("SetStreamSelection", hr));
        safe_release(&reader);
        return false;
    }
    hr = reader->SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM, TRUE);
    if (FAILED(hr)) {
        set_error(&result->error, format_hresult("SetStreamSelection(video)", hr));
        safe_release(&reader);
        return false;
    }

    IMFMediaType* media_type = nullptr;
    hr = reader->GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, &media_type);
    if (FAILED(hr)) {
        set_error(&result->error, format_hresult("GetCurrentMediaType", hr));
        safe_release(&reader);
        return false;
    }

    PROPVARIANT duration_prop;
    PropVariantInit(&duration_prop);
    hr = reader->GetPresentationAttribute(
        MF_SOURCE_READER_MEDIASOURCE,
        MF_PD_DURATION,
        &duration_prop
    );
    UINT64 duration = 0;
    if (SUCCEEDED(hr) && duration_prop.vt == VT_UI8) {
        duration = duration_prop.uhVal.QuadPart;
    }
    PropVariantClear(&duration_prop);

    UINT32 frame_rate_num = 0;
    UINT32 frame_rate_den = 0;
    hr = MFGetAttributeRatio(
        media_type,
        MF_MT_FRAME_RATE,
        &frame_rate_num,
        &frame_rate_den
    );

    if (duration > 0 && SUCCEEDED(hr) && frame_rate_den != 0) {
        double seconds = static_cast<double>(duration) / 10000000.0;
        double fps = static_cast<double>(frame_rate_num) /
                     static_cast<double>(frame_rate_den);
        if (fps > 0.0) {
            uint64_t estimated = static_cast<uint64_t>(std::llround(seconds * fps));
            result->has_value = true;
            result->value = estimated;
        }
    }

    safe_release(&media_type);
    safe_release(&reader);
    return true;
}

bool mft_decode(
    const char* path,
    CMftFrameCallback callback,
    void* context,
    char** out_error
) {
    if (out_error) {
        *out_error = nullptr;
    }
    if (!callback) {
        set_error(out_error, "callback is null");
        return false;
    }

    std::string conversion_error;
    std::wstring wide_path = utf8_to_wide(path, conversion_error);
    if (!conversion_error.empty()) {
        set_error(out_error, conversion_error);
        return false;
    }

    ScopedCoInitialize coinitialize;
    if (!coinitialize.ok()) {
        set_error(out_error, coinitialize.error_message());
        return false;
    }

    ScopedMediaFoundation media_foundation;
    if (!media_foundation.ok()) {
        set_error(out_error, media_foundation.error_message());
        return false;
    }

    IMFAttributes* attributes = nullptr;
    HRESULT hr = MFCreateAttributes(&attributes, 2);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("MFCreateAttributes", hr));
        safe_release(&attributes);
        return false;
    }
    attributes->SetUINT32(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, TRUE);
    attributes->SetUINT32(MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, TRUE);

    IMFSourceReader* reader = nullptr;
    hr = MFCreateSourceReaderFromURL(wide_path.c_str(), attributes, &reader);
    safe_release(&attributes);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("MFCreateSourceReaderFromURL", hr));
        safe_release(&reader);
        return false;
    }

    hr = reader->SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS, FALSE);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("SetStreamSelection", hr));
        safe_release(&reader);
        return false;
    }
    hr = reader->SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM, TRUE);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("SetStreamSelection(video)", hr));
        safe_release(&reader);
        return false;
    }

    IMFMediaType* output_type = nullptr;
    hr = MFCreateMediaType(&output_type);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("MFCreateMediaType", hr));
        safe_release(&reader);
        return false;
    }
    output_type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
    output_type->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12);
    hr = reader->SetCurrentMediaType(
        MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        nullptr,
        output_type
    );
    safe_release(&output_type);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("SetCurrentMediaType", hr));
        safe_release(&reader);
        return false;
    }

    IMFMediaType* current_type = nullptr;
    hr = reader->GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, &current_type);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("GetCurrentMediaType", hr));
        safe_release(&reader);
        return false;
    }
    UINT32 width = 0;
    UINT32 height = 0;
    hr = MFGetAttributeSize(current_type, MF_MT_FRAME_SIZE, &width, &height);
    safe_release(&current_type);
    if (FAILED(hr)) {
        set_error(out_error, format_hresult("MFGetAttributeSize", hr));
        safe_release(&reader);
        return false;
    }

    bool keep_running = true;
    uint64_t frame_index = 0;

    while (keep_running) {
        DWORD stream_index = 0;
        DWORD flags = 0;
        LONGLONG timestamp = 0;
        IMFSample* sample = nullptr;
        hr = reader->ReadSample(
            MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            0,
            &stream_index,
            &flags,
            &timestamp,
            &sample
        );
        if (FAILED(hr)) {
            set_error(out_error, format_hresult("ReadSample", hr));
            safe_release(&sample);
            safe_release(&reader);
            return false;
        }
        if (flags & MF_SOURCE_READERF_ENDOFSTREAM) {
            safe_release(&sample);
            break;
        }
        if (flags & MF_SOURCE_READERF_STREAMTICK) {
            safe_release(&sample);
            continue;
        }
        if (!sample) {
            continue;
        }

        IMFMediaBuffer* buffer = nullptr;
        hr = sample->ConvertToContiguousBuffer(&buffer);
        if (FAILED(hr)) {
            set_error(out_error, format_hresult("ConvertToContiguousBuffer", hr));
            safe_release(&sample);
            safe_release(&buffer);
            safe_release(&reader);
            return false;
        }

        FrameLock lock;
        hr = lock.lock(buffer, width);
        if (FAILED(hr) || !lock.data) {
            set_error(out_error, format_hresult("IMFMediaBuffer::Lock", hr));
            lock.unlock();
            safe_release(&buffer);
            safe_release(&sample);
            safe_release(&reader);
            return false;
        }

        size_t stride = lock.stride >= 0
            ? static_cast<size_t>(lock.stride)
            : static_cast<size_t>(-lock.stride);
        size_t plane_height = static_cast<size_t>(height);
        size_t expected = stride * plane_height;
        size_t available = lock.locked_raw
            ? static_cast<size_t>(lock.contiguous_length)
            : expected;
        size_t plane_bytes = expected <= available ? expected : available;

        CMftFrame frame{};
        frame.data = reinterpret_cast<const uint8_t*>(lock.data);
        frame.data_len = plane_bytes;
        frame.width = width;
        frame.height = height;
        frame.stride = stride;
        frame.timestamp_seconds = timestamp >= 0
            ? static_cast<double>(timestamp) / 10000000.0
            : -1.0;
        frame.frame_index = frame_index;

        keep_running = callback(&frame, context);
        frame_index += 1;

        lock.unlock();
        safe_release(&buffer);
        safe_release(&sample);
    }

    safe_release(&reader);
    return true;
}

void mft_string_free(char* ptr) {
    if (ptr) {
        CoTaskMemFree(ptr);
    }
}

} // extern "C"

#else

extern "C" void mft_string_free(char* ptr);

void mft_string_free(char* ptr) {
    (void)ptr;
}

#endif
