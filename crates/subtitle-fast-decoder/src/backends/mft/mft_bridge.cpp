#ifdef _WIN32

#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601
#endif

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfobjects.h>
#include <mfreadwrite.h>
#include <mferror.h>
#include <propvarutil.h>
#include <combaseapi.h>
#include <wrl/client.h>

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <limits>
#include <cstring>
#include <string>

namespace
{
    using Microsoft::WRL::ComPtr;

    std::string hresult(const char *label, HRESULT hr)
    {
        char buffer[80];
        std::snprintf(buffer, sizeof(buffer), "%s failed: 0x%08lx", label, static_cast<unsigned long>(hr));
        return buffer;
    }

    struct ScopedCoInitialize
    {
        HRESULT result = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        ~ScopedCoInitialize() { if (SUCCEEDED(result)) CoUninitialize(); }
        bool ok() const { return SUCCEEDED(result) || result == RPC_E_CHANGED_MODE; }
        std::string error() const { return hresult("CoInitializeEx", result); }
    };

    struct ScopedMediaFoundation
    {
        HRESULT result = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        ~ScopedMediaFoundation() { if (SUCCEEDED(result)) MFShutdown(); }
        bool ok() const { return SUCCEEDED(result); }
        std::string error() const { return hresult("MFStartup", result); }
    };

    std::wstring utf8_to_wide(const char *utf8, std::string &error)
    {
        if (!utf8) { error = "input path is null"; return {}; }
        int required = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, nullptr, 0);
        if (required <= 1) { error = "failed to convert UTF-8 path to UTF-16"; return {}; }
        std::wstring wide(static_cast<size_t>(required - 1), L'\0');
        if (MultiByteToWideChar(CP_UTF8, 0, utf8, -1, wide.data(), required) != required)
        { error = "failed to convert UTF-8 path to UTF-16"; wide.clear(); }
        return wide;
    }

    char *duplicate_string(const std::string &value)
    {
        char *buffer = static_cast<char *>(CoTaskMemAlloc(value.size() + 1));
        if (buffer) { std::memcpy(buffer, value.c_str(), value.size() + 1); }
        return buffer;
    }

    void set_error(char **out, const std::string &message)
    {
        if (out) { *out = duplicate_string(message); }
    }

    struct FrameLock
    {
        ComPtr<IMFMediaBuffer> buffer;
        ComPtr<IMF2DBuffer> buffer2d;
        BYTE *data = nullptr;
        DWORD contiguous_length = 0;
        LONG stride = 0;

        ~FrameLock() { unlock(); }

        HRESULT lock(IMFMediaBuffer *source_buffer, UINT32 width)
        {
            buffer = source_buffer;
            if (!buffer) { return E_POINTER; }
            HRESULT hr = buffer.As(&buffer2d);
            if (SUCCEEDED(hr) && buffer2d && SUCCEEDED(buffer2d->Lock2D(&data, &stride)))
            {
                DWORD current_length = 0;
                if (SUCCEEDED(buffer->GetCurrentLength(&current_length)))
                {
                    contiguous_length = current_length;
                }
                return S_OK;
            }
            buffer2d.Reset();
            BYTE *raw = nullptr; DWORD max_length = 0;
            hr = buffer->Lock(&raw, &max_length, &contiguous_length);
            if (FAILED(hr)) { return hr; }
            data = raw;
            stride = static_cast<LONG>(width);
            return S_OK;
        }

        void unlock()
        {
            if (!data) { return; }
            if (buffer2d) { buffer2d->Unlock2D(); }
            else if (buffer) { buffer->Unlock(); }
            buffer2d.Reset();
            buffer.Reset();
            data = nullptr;
            contiguous_length = 0;
            stride = 0;
        }
    };

    HRESULT set_format(IMFSourceReader *reader, const GUID &subtype, UINT32 *out_width, UINT32 *out_height, std::string &error)
    {
        ComPtr<IMFMediaType> type;
        HRESULT hr = MFCreateMediaType(&type);
        if (FAILED(hr)) { error = hresult("MFCreateMediaType", hr); return hr; }
        type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
        type->SetGUID(MF_MT_SUBTYPE, subtype);
        hr = reader->SetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), nullptr, type.Get());
        if (FAILED(hr)) { error = hresult("SetCurrentMediaType", hr); return hr; }

        ComPtr<IMFMediaType> current;
        hr = reader->GetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), &current);
        if (FAILED(hr)) { error = hresult("GetCurrentMediaType", hr); return hr; }

        UINT32 width = 0, height = 0;
        hr = MFGetAttributeSize(current.Get(), MF_MT_FRAME_SIZE, &width, &height);
        if (FAILED(hr)) { error = hresult("MFGetAttributeSize", hr); return hr; }

        if (out_width) { *out_width = width; }
        if (out_height) { *out_height = height; }
        return S_OK;
    }

    ComPtr<IMFSourceReader> open_reader(const std::wstring &wide_path, bool enable_video_processing, UINT32 *out_width, UINT32 *out_height, std::string &error)
    {
        ComPtr<IMFAttributes> attributes;
        if (enable_video_processing && FAILED(MFCreateAttributes(&attributes, 1))) { attributes.Reset(); }
        if (attributes) { attributes->SetUINT32(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, TRUE); }

        ComPtr<IMFSourceReader> reader;
        HRESULT hr = MFCreateSourceReaderFromURL(wide_path.c_str(), attributes.Get(), &reader);
        if (FAILED(hr) && hr == E_INVALIDARG) { hr = MFCreateSourceReaderFromURL(wide_path.c_str(), nullptr, &reader); }
        if (FAILED(hr)) { error = hresult("MFCreateSourceReaderFromURL", hr); return {}; }

        hr = reader->SetStreamSelection(static_cast<DWORD>(MF_SOURCE_READER_ALL_STREAMS), FALSE);
        if (FAILED(hr)) { error = hresult("SetStreamSelection", hr); return {}; }
        hr = reader->SetStreamSelection(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), TRUE);
        if (FAILED(hr)) { error = hresult("SetStreamSelection(video)", hr); return {}; }

        std::string format_error;
        if (FAILED(set_format(reader.Get(), MFVideoFormat_NV12, out_width, out_height, format_error)))
        {
            error = std::move(format_error);
            reader.Reset();
        }
        return reader;
    }

    ComPtr<IMFSourceReader> open_best(const std::wstring &path, UINT32 *w, UINT32 *h, std::string &error)
    {
        ComPtr<IMFSourceReader> reader = open_reader(path, true, w, h, error);
        return reader ? reader : open_reader(path, false, w, h, error);
    }

} // namespace

extern "C"
{

    struct CMftProbeResult
    {
        bool has_value;
        uint64_t value;
        double duration_seconds;
        double fps;
        uint32_t width;
        uint32_t height;
        char *error;
    };

    struct CMftFrame
    {
        const uint8_t *y_data;
        size_t y_len;
        size_t y_stride;
        const uint8_t *uv_data;
        size_t uv_len;
        size_t uv_stride;
        uint32_t width;
        uint32_t height;
        double timestamp_seconds;
        uint64_t frame_index;
    };

    typedef bool(__cdecl *CMftFrameCallback)(const CMftFrame *, void *);

    bool mft_probe_total_frames(const char *path, CMftProbeResult *result)
    {
        if (!result) { return false; }
        result->has_value = false;
        result->value = 0;
        result->duration_seconds = std::numeric_limits<double>::quiet_NaN();
        result->fps = std::numeric_limits<double>::quiet_NaN();
        result->width = 0;
        result->height = 0;
        result->error = nullptr;

        std::string conversion_error;
        std::wstring wide_path = utf8_to_wide(path, conversion_error);
        if (!conversion_error.empty())
        {
            set_error(&result->error, conversion_error);
            return false;
        }

        ScopedCoInitialize coinitialize;
        if (!coinitialize.ok())
        {
            set_error(&result->error, coinitialize.error());
            return false;
        }

        ScopedMediaFoundation media_foundation;
        if (!media_foundation.ok())
        {
            set_error(&result->error, media_foundation.error());
            return false;
        }

        std::string reader_error;
        UINT32 width = 0;
        UINT32 height = 0;
        ComPtr<IMFSourceReader> reader = open_best(wide_path, &width, &height, reader_error);
        if (!reader)
        {
            set_error(&result->error, reader_error);
            return false;
        }

        ComPtr<IMFMediaType> media_type;
        HRESULT hr = reader->GetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), &media_type);
        if (FAILED(hr))
        {
            set_error(&result->error, hresult("GetCurrentMediaType", hr));
            return false;
        }

        PROPVARIANT duration_prop;
        PropVariantInit(&duration_prop);
        hr = reader->GetPresentationAttribute(
            static_cast<DWORD>(MF_SOURCE_READER_MEDIASOURCE),
            MF_PD_DURATION,
            &duration_prop);
        UINT64 duration = 0;
        if (SUCCEEDED(hr) && duration_prop.vt == VT_UI8)
        {
            duration = duration_prop.uhVal.QuadPart;
        }
        PropVariantClear(&duration_prop);

        UINT32 frame_rate_num = 0;
        UINT32 frame_rate_den = 0;
        HRESULT fr_hr = MFGetAttributeRatio(
            media_type.Get(),
            MF_MT_FRAME_RATE,
            &frame_rate_num,
            &frame_rate_den);
        
        if (SUCCEEDED(fr_hr) && frame_rate_num > 0 && frame_rate_den > 0 && duration > 0)
        {
            UINT64 frame_duration = static_cast<UINT64>(
                (static_cast<double>(frame_rate_den) / static_cast<double>(frame_rate_num)) * 10000000.0);
            
            UINT64 seek_offset = 1 * 10000000ULL;
            if (duration > seek_offset)
            {
                PROPVARIANT seek_pos;
                PropVariantInit(&seek_pos);
                seek_pos.vt = VT_I8;
                seek_pos.hVal.QuadPart = static_cast<LONGLONG>(duration - seek_offset);
                
                HRESULT seek_hr = reader->SetCurrentPosition(GUID_NULL, seek_pos);
                PropVariantClear(&seek_pos);
                
                if (SUCCEEDED(seek_hr))
                {
                    LONGLONG last_timestamp = 0;
                    bool found_frame = false;
                    
                    for (int max_reads = 0; max_reads < 2000; ++max_reads)
                    {
                        DWORD stream_index = 0;
                        DWORD flags = 0;
                        LONGLONG timestamp = 0;
                        ComPtr<IMFSample> sample;
                        HRESULT rd_hr = reader->ReadSample(
                            static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM),
                            0,
                            &stream_index,
                            &flags,
                            &timestamp,
                            &sample);
                        
                        if (FAILED(rd_hr) || (flags & MF_SOURCE_READERF_ENDOFSTREAM))
                        {
                            break;
                        }
                        
                        if (sample && timestamp >= 0)
                        {
                            last_timestamp = timestamp;
                            found_frame = true;
                        }
                    }
                    
                    if (found_frame && last_timestamp > 0)
                    {
                        duration = static_cast<UINT64>(last_timestamp) + frame_duration;
                    }
                }
            }
        }


        if (width > 0) { result->width = width; }
        if (height > 0) { result->height = height; }

        double seconds = std::numeric_limits<double>::quiet_NaN();
        if (duration > 0)
        {
            seconds = static_cast<double>(duration) / 10000000.0;
            if (std::isfinite(seconds) && seconds > 0.0)
            {
                result->duration_seconds = seconds;
            }
            else
            {
                seconds = std::numeric_limits<double>::quiet_NaN();
            }
        }

        double fps = std::numeric_limits<double>::quiet_NaN();
        if (SUCCEEDED(fr_hr) && frame_rate_den != 0)
        {
            fps = static_cast<double>(frame_rate_num) / static_cast<double>(frame_rate_den);
            if (std::isfinite(fps) && fps > 0.0)
            {
                result->fps = fps;
            }
            else
            {
                fps = std::numeric_limits<double>::quiet_NaN();
            }
        }

        if (std::isfinite(seconds) && seconds > 0.0 && std::isfinite(fps) && fps > 0.0)
        {
            uint64_t estimated = static_cast<uint64_t>(std::llround(seconds * fps));
            if (estimated > 0)
            {
                result->has_value = true;
                result->value = estimated;
            }
        }
        return true;
    }

    bool mft_decode(
        const char *path,
        CMftFrameCallback callback,
        void *context,
        char **out_error)
    {
        if (out_error) { *out_error = nullptr; }
        if (!callback)
        {
            set_error(out_error, "callback is null");
            return false;
        }

        std::string conversion_error;
        std::wstring wide_path = utf8_to_wide(path, conversion_error);
        if (!conversion_error.empty())
        {
            set_error(out_error, conversion_error);
            return false;
        }

        ScopedCoInitialize coinitialize;
        if (!coinitialize.ok())
        {
            set_error(out_error, coinitialize.error());
            return false;
        }

        ScopedMediaFoundation media_foundation;
        if (!media_foundation.ok())
        {
            set_error(out_error, media_foundation.error());
            return false;
        }

        std::string reader_error;
        UINT32 width = 0, height = 0;
        ComPtr<IMFSourceReader> reader = open_best(wide_path, &width, &height, reader_error);
        if (!reader)
        {
            set_error(out_error, reader_error);
            return false;
        }

        for (uint64_t frame_index = 0;; frame_index++)
        {
            DWORD stream_index = 0;
            DWORD flags = 0;
            LONGLONG timestamp = 0;
            ComPtr<IMFSample> sample;
            HRESULT hr = reader->ReadSample(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), 0, &stream_index, &flags, &timestamp, &sample);
            if (FAILED(hr))
            {
                set_error(out_error, hresult("ReadSample", hr));
                return false;
            }
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM)
            {
                break;
            }
            if ((flags & MF_SOURCE_READERF_STREAMTICK) || !sample) { continue; }

            ComPtr<IMFMediaBuffer> buffer;
            hr = sample->ConvertToContiguousBuffer(&buffer);
            if (FAILED(hr) || !buffer)
            {
                set_error(out_error, hresult("ConvertToContiguousBuffer", hr));
                return false;
            }

            FrameLock lock;
            hr = lock.lock(buffer.Get(), width);
            if (FAILED(hr) || !lock.data)
            {
                set_error(out_error, hresult("IMFMediaBuffer::Lock", hr));
                return false;
            }

            if (!lock.data)
            {
                set_error(out_error, "MFT buffer missing NV12 data");
                return false;
            }

            size_t stride = static_cast<size_t>(lock.stride >= 0 ? lock.stride : -lock.stride);
            size_t y_rows = static_cast<size_t>(height);
            size_t uv_rows = (y_rows + 1) / 2;
            if (stride == 0 || y_rows == 0)
            {
                set_error(out_error, "invalid stride or height for NV12 frame");
                return false;
            }

            if (stride > (std::numeric_limits<size_t>::max)() / (y_rows + uv_rows))
            {
                set_error(out_error, "NV12 plane length overflow");
                return false;
            }

            size_t y_len = stride * y_rows;
            size_t uv_len = stride * uv_rows;
            size_t total_len = y_len + uv_len;
            size_t available = static_cast<size_t>(lock.contiguous_length);
            if (available < total_len)
            {
                set_error(out_error, "MFT buffer missing NV12 UV plane data");
                return false;
            }

            CMftFrame frame{};
            frame.y_data = reinterpret_cast<const uint8_t *>(lock.data);
            frame.y_len = y_len;
            frame.y_stride = stride;
            frame.uv_data = reinterpret_cast<const uint8_t *>(lock.data) + y_len;
            frame.uv_len = uv_len;
            frame.uv_stride = stride;
            frame.width = width;
            frame.height = height;
            frame.timestamp_seconds = timestamp >= 0
                                          ? static_cast<double>(timestamp) / 10000000.0
                                          : -1.0;
            frame.frame_index = frame_index;

            if (!callback(&frame, context)) { break; }
        }

        return true;
    }

    void mft_string_free(char *ptr)
    {
        if (ptr)
        {
            CoTaskMemFree(ptr);
        }
    }

} // extern "C"

#else

extern "C" void mft_string_free(char *ptr);

void mft_string_free(char *ptr)
{
    (void)ptr;
}

#endif
