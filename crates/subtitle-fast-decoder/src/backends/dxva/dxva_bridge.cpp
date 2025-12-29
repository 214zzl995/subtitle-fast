#ifdef _WIN32

#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601
#endif

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11.h>
#include <d3d11_4.h>
#include <dxgi1_2.h>
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
#include <cstring>
#include <cstdlib>
#include <limits>
#include <string>
#include <vector>

namespace
{
    using Microsoft::WRL::ComPtr;

    std::string hresult(const char *label, HRESULT hr)
    {
        char buffer[96];
        std::snprintf(buffer, sizeof(buffer), "%s failed: 0x%08lx", label, static_cast<unsigned long>(hr));
        return buffer;
    }

    bool compute_seek_timestamp(
        uint64_t start_frame,
        UINT32 frame_rate_num,
        UINT32 frame_rate_den,
        LONGLONG &out_value,
        std::string &error)
    {
        if (frame_rate_num == 0 || frame_rate_den == 0)
        {
            error = "DXVA requires frame rate metadata to seek";
            return false;
        }

        long double frames = static_cast<long double>(start_frame);
        long double fps_num = static_cast<long double>(frame_rate_num);
        long double fps_den = static_cast<long double>(frame_rate_den);
        long double seconds = frames * fps_den / fps_num;
        long double ticks = seconds * 10000000.0L;
        if (!std::isfinite(ticks) || ticks < 0.0L)
        {
            error = "start frame timestamp overflow";
            return false;
        }

        long double max_value = static_cast<long double>((std::numeric_limits<LONGLONG>::max)());
        if (ticks > max_value)
        {
            error = "start frame timestamp overflow";
            return false;
        }

        out_value = static_cast<LONGLONG>(std::llround(ticks));
        return true;
    }

    std::string wide_to_utf8(const wchar_t *wide)
    {
        if (!wide) { return {}; }
        int required = WideCharToMultiByte(CP_UTF8, 0, wide, -1, nullptr, 0, nullptr, nullptr);
        if (required <= 1) { return {}; }
        std::string utf8(static_cast<size_t>(required - 1), '\0');
        if (WideCharToMultiByte(CP_UTF8, 0, wide, -1, utf8.data(), required, nullptr, nullptr) != required)
        {
            return {};
        }
        return utf8;
    }

    bool select_adapter(Microsoft::WRL::ComPtr<IDXGIAdapter1> &out, std::string &description, std::string &error);

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

    struct D3D11Context
    {
        ComPtr<ID3D11Device> device;
        ComPtr<ID3D11DeviceContext> context;
        ComPtr<IMFDXGIDeviceManager> device_manager;
        UINT reset_token = 0;
        std::string adapter_description;

        bool initialize(std::string &error)
        {
            ComPtr<IDXGIAdapter1> adapter;
            if (!select_adapter(adapter, adapter_description, error))
            {
                return false;
            }

            UINT flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
            const D3D_FEATURE_LEVEL levels[] = {
#if defined(D3D_FEATURE_LEVEL_12_1)
                D3D_FEATURE_LEVEL_12_1,
#endif
#if defined(D3D_FEATURE_LEVEL_12_0)
                D3D_FEATURE_LEVEL_12_0,
#endif
#if defined(D3D_FEATURE_LEVEL_11_1)
                D3D_FEATURE_LEVEL_11_1,
#endif
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_10_0,
                D3D_FEATURE_LEVEL_9_3,
                D3D_FEATURE_LEVEL_9_2,
                D3D_FEATURE_LEVEL_9_1,
            };
            HRESULT hr = D3D11CreateDevice(
                adapter.Get(),
                adapter ? D3D_DRIVER_TYPE_UNKNOWN : D3D_DRIVER_TYPE_HARDWARE,
                nullptr,
                flags,
                levels,
                ARRAYSIZE(levels),
                D3D11_SDK_VERSION,
                &device,
                nullptr,
                &context);
            if (FAILED(hr))
            {
                error = hresult("D3D11CreateDevice", hr);
                return false;
            }

            ComPtr<IMFDXGIDeviceManager> manager;
            hr = MFCreateDXGIDeviceManager(&reset_token, &manager);
            if (FAILED(hr))
            {
                error = hresult("MFCreateDXGIDeviceManager", hr);
                return false;
            }
            hr = manager->ResetDevice(device.Get(), reset_token);
            if (FAILED(hr))
            {
                error = hresult("IMFDXGIDeviceManager::ResetDevice", hr);
                return false;
            }

            device_manager = manager;

            #if defined(__ID3D11Multithread_INTERFACE_DEFINED__)
            ComPtr<ID3D11Multithread> multithread;
            if (SUCCEEDED(device.As(&multithread)) && multithread)
            {
                multithread->SetMultithreadProtected(TRUE);
            }
            #endif
            return true;
        }
    };

    bool parse_vendor_from_env(UINT &vendor_out)
    {
        char buffer[16] = {0};
        DWORD read = GetEnvironmentVariableA("SUBFAST_DXVA_ADAPTER_VENDOR", buffer, static_cast<DWORD>(sizeof(buffer)));
        if (read == 0 || read >= sizeof(buffer)) { return false; }
        char *end = nullptr;
        unsigned long value = std::strtoul(buffer, &end, 0);
        if (end == buffer || value > 0xFFFFFFFFul) { return false; }
        vendor_out = static_cast<UINT>(value);
        return true;
    }

    bool select_adapter(ComPtr<IDXGIAdapter1> &out, std::string &description, std::string &error)
    {
        ComPtr<IDXGIFactory1> factory;
        HRESULT hr = CreateDXGIFactory1(IID_PPV_ARGS(&factory));
        if (FAILED(hr))
        {
            error = hresult("CreateDXGIFactory1", hr);
            return false;
        }

        UINT desired_vendor = 0;
        bool vendor_requested = parse_vendor_from_env(desired_vendor);

        UINT index = 0;
        SIZE_T best_memory = 0;
        ComPtr<IDXGIAdapter1> best;
        DXGI_ADAPTER_DESC1 best_desc{};

        while (true)
        {
            ComPtr<IDXGIAdapter1> adapter;
            hr = factory->EnumAdapters1(index++, &adapter);
            if (hr == DXGI_ERROR_NOT_FOUND) { break; }
            if (FAILED(hr)) { continue; }

            DXGI_ADAPTER_DESC1 desc{};
            if (FAILED(adapter->GetDesc1(&desc))) { continue; }
            if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) { continue; }

            if (vendor_requested && desc.VendorId == desired_vendor)
            {
                out = adapter;
                description = wide_to_utf8(desc.Description);
                return true;
            }

            if (desc.DedicatedVideoMemory > best_memory)
            {
                best = adapter;
                best_desc = desc;
                best_memory = desc.DedicatedVideoMemory;
            }
        }

        if (best)
        {
            out = best;
            description = wide_to_utf8(best_desc.Description);
            return true;
        }

        // No suitable adapter found; let D3D pick default hardware.
        description.clear();
        return true;
    }

    struct StagingCopy
    {
        ComPtr<ID3D11Texture2D> texture;
        UINT width = 0;
        UINT height = 0;
        DXGI_FORMAT format = DXGI_FORMAT_UNKNOWN;

        HRESULT ensure(ID3D11Device *device, UINT target_width, UINT target_height, DXGI_FORMAT target_format)
        {
            if (texture && width == target_width && height == target_height && format == target_format)
            {
                return S_OK;
            }
            D3D11_TEXTURE2D_DESC desc{};
            desc.Width = target_width;
            desc.Height = target_height;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            desc.Format = target_format;
            desc.SampleDesc.Count = 1;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
            desc.MiscFlags = 0;

            ComPtr<ID3D11Texture2D> staging;
            HRESULT hr = device->CreateTexture2D(&desc, nullptr, &staging);
            if (FAILED(hr))
            {
                return hr;
            }
            texture = staging;
            width = target_width;
            height = target_height;
            format = target_format;
            return S_OK;
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

    ComPtr<IMFSourceReader> open_reader(const std::wstring &wide_path, D3D11Context &d3d, bool enable_video_processing, UINT32 *out_width, UINT32 *out_height, std::string &error)
    {
        ComPtr<IMFAttributes> attributes;
        if (SUCCEEDED(MFCreateAttributes(&attributes, 4)))
        {
            attributes->SetUINT32(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, enable_video_processing ? TRUE : FALSE);
            attributes->SetUINT32(MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, TRUE);
            attributes->SetUnknown(MF_SOURCE_READER_D3D_MANAGER, d3d.device_manager.Get());
        }

        ComPtr<IMFSourceReader> reader;
        HRESULT hr = MFCreateSourceReaderFromURL(wide_path.c_str(), attributes.Get(), &reader);
        if (FAILED(hr) && hr == E_INVALIDARG)
        {
            hr = MFCreateSourceReaderFromURL(wide_path.c_str(), nullptr, &reader);
        }
        if (FAILED(hr))
        {
            error = hresult("MFCreateSourceReaderFromURL", hr);
            return {};
        }

        hr = reader->SetStreamSelection(static_cast<DWORD>(MF_SOURCE_READER_ALL_STREAMS), FALSE);
        if (FAILED(hr)) { error = hresult("SetStreamSelection", hr); return {}; }
        hr = reader->SetStreamSelection(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), TRUE);
        if (FAILED(hr)) { error = hresult("SetStreamSelection(video)", hr); return {}; }

        // Require NV12; reject other formats to avoid silent CPU paths.
        std::string format_error;
        if (FAILED(set_format(reader.Get(), MFVideoFormat_NV12, out_width, out_height, format_error)))
        {
            error = std::move(format_error);
            reader.Reset();
        }
        return reader;
    }

    ComPtr<IMFSourceReader> open_best(const std::wstring &path, D3D11Context &d3d, UINT32 *w, UINT32 *h, std::string &error)
    {
        // Try without video processing first to keep surfaces on GPU; fall back to enabling processing only if needed.
        ComPtr<IMFSourceReader> reader = open_reader(path, d3d, false, w, h, error);
        return reader ? reader : open_reader(path, d3d, true, w, h, error);
    }

    bool copy_frame_gpu(
        IMFDXGIBuffer *dxgi_buffer,
        D3D11Context &d3d,
        StagingCopy &staging,
        UINT height,
        UINT uv_rows,
        std::vector<uint8_t> &out,
        size_t &stride,
        std::string &error)
    {
        if (!dxgi_buffer) { error = "DXGI buffer is null"; return false; }

        ComPtr<ID3D11Texture2D> texture;
        UINT subresource = 0;
        HRESULT hr = dxgi_buffer->GetResource(IID_PPV_ARGS(&texture));
        if (FAILED(hr) || !texture)
        {
            error = hresult("IMFDXGIBuffer::GetResource", hr);
            return false;
        }
        hr = dxgi_buffer->GetSubresourceIndex(&subresource);
        if (FAILED(hr))
        {
            error = hresult("IMFDXGIBuffer::GetSubresourceIndex", hr);
            return false;
        }

        D3D11_TEXTURE2D_DESC desc{};
        texture->GetDesc(&desc);
        hr = staging.ensure(d3d.device.Get(), desc.Width, desc.Height, desc.Format);
        if (FAILED(hr))
        {
            error = hresult("ID3D11Device::CreateTexture2D", hr);
            return false;
        }

        d3d.context->CopySubresourceRegion(staging.texture.Get(), 0, 0, 0, 0, texture.Get(), subresource, nullptr);

        D3D11_MAPPED_SUBRESOURCE mapped{};
        hr = d3d.context->Map(staging.texture.Get(), 0, D3D11_MAP_READ, 0, &mapped);
        if (FAILED(hr))
        {
            error = hresult("ID3D11DeviceContext::Map", hr);
            return false;
        }

        stride = static_cast<size_t>(mapped.RowPitch);
        const size_t y_rows = static_cast<size_t>(height);
        const size_t uv_plane_rows = static_cast<size_t>(uv_rows);
        const size_t total_rows = y_rows + uv_plane_rows;
        if (stride == 0 || y_rows == 0 || stride > (std::numeric_limits<size_t>::max)() / total_rows)
        {
            d3d.context->Unmap(staging.texture.Get(), 0);
            error = "invalid stride when copying DXVA frame";
            return false;
        }

        const size_t required = stride * total_rows;
        out.resize(required);

        const uint8_t *src = static_cast<const uint8_t *>(mapped.pData);
        for (size_t row = 0; row < total_rows; ++row)
        {
            std::memcpy(out.data() + row * stride, src + row * mapped.RowPitch, stride);
        }

        d3d.context->Unmap(staging.texture.Get(), 0);
        return true;
    }

} // namespace

extern "C"
{

    struct CDxvaProbeResult
    {
        bool has_value;
        uint64_t value;
        double duration_seconds;
        double fps;
        uint32_t width;
        uint32_t height;
        char *error;
    };

    struct CDxvaFrame
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

    typedef bool(__cdecl *CDxvaFrameCallback)(const CDxvaFrame *, void *);

    bool dxva_probe_total_frames(const char *path, CDxvaProbeResult *result)
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

        D3D11Context d3d;
        std::string device_error;
        if (!d3d.initialize(device_error))
        {
            set_error(&result->error, device_error);
            return false;
        }

        std::string reader_error;
        UINT32 width = 0;
        UINT32 height = 0;
        ComPtr<IMFSourceReader> reader = open_best(wide_path, d3d, &width, &height, reader_error);
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

    bool dxva_decode(
        const char *path,
        bool has_start_frame,
        uint64_t start_frame,
        CDxvaFrameCallback callback,
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

        D3D11Context d3d;
        std::string device_error;
        if (!d3d.initialize(device_error))
        {
            set_error(out_error, device_error);
            return false;
        }

        std::string reader_error;
        UINT32 width = 0, height = 0;
        ComPtr<IMFSourceReader> reader = open_best(wide_path, d3d, &width, &height, reader_error);
        if (!reader)
        {
            set_error(out_error, reader_error);
            return false;
        }

        if (has_start_frame)
        {
            ComPtr<IMFMediaType> media_type;
            HRESULT mt_hr = reader->GetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM), &media_type);
            if (FAILED(mt_hr))
            {
                set_error(out_error, hresult("GetCurrentMediaType", mt_hr));
                return false;
            }

            UINT32 frame_rate_num = 0;
            UINT32 frame_rate_den = 0;
            HRESULT fr_hr = MFGetAttributeRatio(media_type.Get(), MF_MT_FRAME_RATE, &frame_rate_num, &frame_rate_den);
            if (FAILED(fr_hr))
            {
                set_error(out_error, hresult("MFGetAttributeRatio", fr_hr));
                return false;
            }

            LONGLONG position_value = 0;
            std::string position_error;
            if (!compute_seek_timestamp(start_frame, frame_rate_num, frame_rate_den, position_value, position_error))
            {
                set_error(out_error, position_error);
                return false;
            }

            PROPVARIANT position;
            PropVariantInit(&position);
            position.vt = VT_I8;
            position.hVal.QuadPart = position_value;
            HRESULT seek_hr = reader->SetCurrentPosition(GUID_NULL, position);
            PropVariantClear(&position);
            if (FAILED(seek_hr))
            {
                set_error(out_error, hresult("SetCurrentPosition", seek_hr));
                return false;
            }
        }

        StagingCopy staging;
        std::vector<uint8_t> plane;
        size_t stride = 0;
        UINT uv_rows = (height + 1) / 2;

        uint64_t start_index = has_start_frame ? start_frame : 0;
        for (uint64_t frame_index = start_index;; frame_index++)
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
            hr = sample->GetBufferByIndex(0, &buffer);
            if (FAILED(hr) || !buffer)
            {
                set_error(out_error, hresult("IMFSample::GetBufferByIndex", hr));
                return false;
            }

            ComPtr<IMFDXGIBuffer> dxgi_buffer;
            HRESULT dxgi_hr = buffer.As(&dxgi_buffer);
            plane.clear();

            if (FAILED(dxgi_hr))
            {
                set_error(out_error, hresult("IMFMediaBuffer::QueryInterface(IMFDXGIBuffer)", dxgi_hr));
                return false;
            }
            if (!dxgi_buffer)
            {
                set_error(out_error, "DXVA sample missing IMFDXGIBuffer surface");
                return false;
            }

            std::string copy_error;
            if (!copy_frame_gpu(dxgi_buffer.Get(), d3d, staging, height, uv_rows, plane, stride, copy_error))
            {
                set_error(out_error, copy_error.empty() ? "failed to copy DXVA surface to CPU" : copy_error);
                return false;
            }

            const size_t y_len = stride * static_cast<size_t>(height);
            const size_t uv_len = stride * static_cast<size_t>(uv_rows);

            CDxvaFrame frame{};
            frame.y_data = plane.data();
            frame.y_len = y_len;
            frame.y_stride = stride;
            frame.uv_data = plane.data() + y_len;
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

    void dxva_string_free(char *ptr)
    {
        if (ptr)
        {
            CoTaskMemFree(ptr);
        }
    }

} // extern "C"

#else

extern "C" void dxva_string_free(char *ptr);

void dxva_string_free(char *ptr)
{
    (void)ptr;
}

#endif
