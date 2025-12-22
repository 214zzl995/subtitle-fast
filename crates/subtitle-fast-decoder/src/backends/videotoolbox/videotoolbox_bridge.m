#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <CoreFoundation/CoreFoundation.h>

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <limits.h>

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"

typedef struct {
    bool has_value;
    uint64_t value;
    double duration_seconds;
    double fps;
    uint32_t width;
    uint32_t height;
    char *error;
} VideoToolboxProbeResult;

typedef struct {
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
} VideoToolboxFrame;

typedef bool (*VideoToolboxFrameCallback)(const VideoToolboxFrame *frame, void *context);

static char *vt_copy_c_string(const char *message) {
    if (message == NULL) {
        return NULL;
    }
    size_t len = strlen(message);
    char *copy = malloc(len + 1);
    if (copy == NULL) {
        return NULL;
    }
    memcpy(copy, message, len);
    copy[len] = '\0';
    return copy;
}

static char *vt_copy_nsstring(NSString *string, const char *fallback) {
    if (string == nil) {
        return fallback != NULL ? vt_copy_c_string(fallback) : NULL;
    }
    const char *utf8 = string.UTF8String;
    if (utf8 == NULL) {
        return fallback != NULL ? vt_copy_c_string(fallback) : NULL;
    }
    return vt_copy_c_string(utf8);
}

static char *vt_copy_error_message(NSError *error, const char *context) {
    if (error == nil) {
        return context != NULL ? vt_copy_c_string(context) : NULL;
    }

    NSMutableString *message = [NSMutableString string];
    if (context != NULL) {
        [message appendString:[NSString stringWithUTF8String:context]];
    }

    NSString *description = error.localizedDescription;
    if (description != nil && description.length > 0) {
        if (message.length > 0) {
            [message appendString:@": "];
        }
        [message appendString:description];
    }

    NSString *domain = error.domain;
    if (domain != nil && domain.length > 0) {
        [message appendFormat:@" (domain=%@ code=%ld)", domain, (long)error.code];
    }

    NSString *reason = error.localizedFailureReason;
    if (reason != nil && reason.length > 0) {
        [message appendFormat:@" reason=%@", reason];
    }

    NSString *suggestion = error.localizedRecoverySuggestion;
    if (suggestion != nil && suggestion.length > 0) {
        [message appendFormat:@" suggestion=%@", suggestion];
    }

    if (message.length == 0) {
        return NULL;
    }

    return vt_copy_nsstring(message, context);
}

static NSString *vt_string_from_utf8(const char *c_string) {
    if (c_string == NULL) {
        return nil;
    }
    return [[NSString alloc] initWithUTF8String:c_string];
}

static bool vt_populate_asset(NSString *path, NSURL **out_url, AVURLAsset **out_asset, char **out_error) {
    if (path == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("failed to convert path to NSString");
        }
        return false;
    }

    NSURL *url = [NSURL fileURLWithPath:path];
    if (url == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("failed to create NSURL for video file");
        }
        return false;
    }

    AVURLAsset *asset = [[AVURLAsset alloc] initWithURL:url options:nil];
    if (asset == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("failed to open AVURLAsset");
        }
        return false;
    }

    if (out_url != NULL) {
        *out_url = url;
    }
    if (out_asset != NULL) {
        *out_asset = asset;
    }
    return true;
}

static bool vt_prepare_reader(AVURLAsset *asset, AVAssetReader **out_reader, AVAssetReaderTrackOutput **out_output, char **out_error) {
    if (asset == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("missing AVURLAsset for reader setup");
        }
        return false;
    }

    NSArray<AVAssetTrack *> *tracks = [asset tracksWithMediaType:AVMediaTypeVideo];
    if (tracks == nil || tracks.count == 0) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("asset contains no video tracks");
        }
        return false;
    }

    AVAssetTrack *track = tracks.firstObject;
    if (track == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("asset contains no primary video track");
        }
        return false;
    }

    const uint32_t pixel_format_nv12 = 875704438;
    NSNumber *pixel_format = [NSNumber numberWithUnsignedInt:pixel_format_nv12];
    if (pixel_format == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("failed to create NSNumber for pixel format");
        }
        return false;
    }

    NSDictionary *settings = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : pixel_format
    };
    if (settings == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("failed to build output settings dictionary");
        }
        return false;
    }

    NSError *reader_error = nil;
    AVAssetReader *reader = [[AVAssetReader alloc] initWithAsset:asset error:&reader_error];
    if (reader == nil || reader_error != nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_error_message(reader_error, "failed to create AVAssetReader");
        }
        return false;
    }

    AVAssetReaderTrackOutput *output = [[AVAssetReaderTrackOutput alloc] initWithTrack:track outputSettings:settings];
    if (output == nil) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("failed to create AVAssetReaderTrackOutput");
        }
        return false;
    }

    if ([output respondsToSelector:@selector(setAlwaysCopiesSampleData:)]) {
        output.alwaysCopiesSampleData = NO;
    }

    if (![reader canAddOutput:output]) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("AVAssetReader refused track output");
        }
        return false;
    }

    [reader addOutput:output];

    if (out_reader != NULL) {
        *out_reader = reader;
    }
    if (out_output != NULL) {
        *out_output = output;
    }

    return true;
}

static bool vt_reader_handle_status(AVAssetReader *reader, char **out_error) {
    AVAssetReaderStatus status = reader.status;
    if (status == AVAssetReaderStatusCompleted) {
        return true;
    } else if (status == AVAssetReaderStatusReading || status == AVAssetReaderStatusUnknown) {
        return true;
    } else if (status == AVAssetReaderStatusCancelled) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("videotoolbox reader was cancelled");
        }
        return false;
    }

    NSError *error = reader.error;
    if (out_error != NULL) {
        *out_error = vt_copy_error_message(error, "videotoolbox reader failed");
    }
    return false;
}

bool videotoolbox_probe_total_frames(const char *path, VideoToolboxProbeResult *out_result) {
    if (out_result == NULL) {
        return false;
    }
    out_result->has_value = false;
    out_result->value = 0;
    out_result->duration_seconds = NAN;
    out_result->fps = NAN;
    out_result->width = 0;
    out_result->height = 0;
    out_result->error = NULL;

    @autoreleasepool {
        NSString *ns_path = vt_string_from_utf8(path);
        NSURL *url = nil;
        AVURLAsset *asset = nil;
        if (!vt_populate_asset(ns_path, &url, &asset, &out_result->error)) {
            return false;
        }

        NSArray<AVAssetTrack *> *tracks = [asset tracksWithMediaType:AVMediaTypeVideo];
        if (tracks == nil || tracks.count == 0) {
            out_result->error = vt_copy_c_string("asset contains no video tracks");
            return false;
        }

        AVAssetTrack *track = tracks.firstObject;
        if (track == nil) {
            out_result->error = vt_copy_c_string("asset contains no primary video track");
            return false;
        }

        CMTimeRange time_range = track.timeRange;
        Float64 duration_seconds = CMTimeGetSeconds(time_range.duration);
        if (isfinite(duration_seconds) && duration_seconds > 0.0) {
            out_result->duration_seconds = duration_seconds;
        } else {
            duration_seconds = NAN;
        }

        Float64 fps = track.nominalFrameRate;
        if (!isfinite(fps) || fps <= 0.0) {
            CMTime min_frame_duration = track.minFrameDuration;
            Float64 frame_duration_seconds = CMTimeGetSeconds(min_frame_duration);
            if (frame_duration_seconds > 0.0 && isfinite(frame_duration_seconds)) {
                fps = 1.0 / frame_duration_seconds;
            }
        }

        if (isfinite(fps) && fps > 0.0) {
            out_result->fps = fps;
        } else {
            fps = NAN;
        }

        CGSize size = track.naturalSize;
        CGFloat width = fabs(size.width);
        CGFloat height = fabs(size.height);
        if (isfinite(width) && width > 0.0) {
            out_result->width = (uint32_t)llround(width);
        }
        if (isfinite(height) && height > 0.0) {
            out_result->height = (uint32_t)llround(height);
        }

        if (isfinite(duration_seconds) && duration_seconds > 0.0 && isfinite(fps) && fps > 0.0) {
            Float64 total = round(duration_seconds * fps);
            if (isfinite(total) && total > 0.0) {
                out_result->has_value = true;
                out_result->value = (uint64_t)total;
            }
        }
    }

    return true;
}

bool videotoolbox_decode(
    const char *path,
    VideoToolboxFrameCallback callback,
    void *context,
    char **out_error
) {
    if (out_error != NULL) {
        *out_error = NULL;
    }
    if (callback == NULL) {
        if (out_error != NULL) {
            *out_error = vt_copy_c_string("videotoolbox frame callback is null");
        }
        return false;
    }

    @autoreleasepool {
        NSString *ns_path = vt_string_from_utf8(path);
        NSURL *url = nil;
        AVURLAsset *asset = nil;
        char *asset_error = NULL;
        if (!vt_populate_asset(ns_path, &url, &asset, &asset_error)) {
            if (out_error != NULL) {
                *out_error = asset_error;
            } else {
                free(asset_error);
            }
            return false;
        }

        AVAssetReader *reader = nil;
        AVAssetReaderTrackOutput *output = nil;
        char *reader_error = NULL;
        if (!vt_prepare_reader(asset, &reader, &output, &reader_error)) {
            if (out_error != NULL) {
                *out_error = reader_error;
            } else {
                free(reader_error);
            }
            return false;
        }

        if (![reader startReading]) {
            if (out_error != NULL) {
                *out_error = vt_copy_error_message(reader.error, "failed to start AVAssetReader");
            }
            return false;
        }

        uint64_t frame_index = 0;

        while (true) {
            CMSampleBufferRef sample = [output copyNextSampleBuffer];
            if (sample == NULL) {
                if (!vt_reader_handle_status(reader, out_error)) {
                    return false;
                }
                if (reader.status == AVAssetReaderStatusCompleted) {
                    break;
                }
                continue;
            }

            CVImageBufferRef pixel_buffer = CMSampleBufferGetImageBuffer(sample);
            if (pixel_buffer == NULL) {
                if (out_error != NULL) {
                    *out_error = vt_copy_c_string("sample buffer missing pixel buffer");
                }
                CFRelease(sample);
                return false;
            }

            size_t plane_count = CVPixelBufferGetPlaneCount(pixel_buffer);
            if (plane_count < 2) {
                if (out_error != NULL) {
                    *out_error = vt_copy_c_string("expected NV12 pixel buffer with Y and UV planes");
                }
                CFRelease(sample);
                return false;
            }

            CVReturn lock_status = CVPixelBufferLockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
            if (lock_status != kCVReturnSuccess) {
                if (out_error != NULL) {
                    *out_error = vt_copy_c_string("failed to lock pixel buffer");
                }
                CFRelease(sample);
                return false;
            }

            const uint8_t *y_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0);
            const uint8_t *uv_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1);
            size_t y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0);
            size_t uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1);
            size_t width = CVPixelBufferGetWidthOfPlane(pixel_buffer, 0);
            size_t height = CVPixelBufferGetHeightOfPlane(pixel_buffer, 0);
            size_t uv_height = CVPixelBufferGetHeightOfPlane(pixel_buffer, 1);

            if (y_base == NULL || uv_base == NULL) {
                if (out_error != NULL) {
                    *out_error = vt_copy_c_string("failed to access NV12 plane data");
                }
                CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
                CFRelease(sample);
                return false;
            }

            if (height != 0 && y_stride > SIZE_MAX / height) {
                if (out_error != NULL) {
                    *out_error = vt_copy_c_string("calculated stride overflow for Y plane");
                }
                CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
                CFRelease(sample);
                return false;
            }
            if (uv_height != 0 && uv_stride > SIZE_MAX / uv_height) {
                if (out_error != NULL) {
                    *out_error = vt_copy_c_string("calculated stride overflow for UV plane");
                }
                CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
                CFRelease(sample);
                return false;
            }

            size_t y_len = y_stride * height;
            size_t uv_len = uv_stride * uv_height;
            CMTime pts = CMSampleBufferGetPresentationTimeStamp(sample);
            Float64 timestamp_seconds = NAN;
            if (pts.timescale != 0) {
                timestamp_seconds = (Float64)pts.value / (Float64)pts.timescale;
            }

            VideoToolboxFrame frame;
            frame.y_data = y_base;
            frame.y_len = y_len;
            frame.y_stride = y_stride;
            frame.uv_data = uv_base;
            frame.uv_len = uv_len;
            frame.uv_stride = uv_stride;
            frame.width = (uint32_t)width;
            frame.height = (uint32_t)height;
            frame.timestamp_seconds = timestamp_seconds;
            frame.frame_index = frame_index;

            bool should_continue = callback(&frame, context);

            CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
            CFRelease(sample);

            if (!should_continue) {
                break;
            }

            frame_index += 1;
        }
    }

    return true;
}

void videotoolbox_string_free(char *ptr) {
    if (ptr != NULL) {
        free(ptr);
    }
}

#pragma clang diagnostic pop
