#import <Foundation/Foundation.h>
#import <Vision/Vision.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreFoundation/CoreFoundation.h>

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

typedef struct {
    float x;
    float y;
    float width;
    float height;
    float confidence;
} VisionRegion;

typedef struct {
    VisionRegion *regions;
    size_t count;
    char *error;
} VisionResult;

static char *vision_copy_c_string(const char *message) {
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

static char *vision_copy_nsstring(NSError *error, const char *fallback) {
    if (error == nil) {
        return vision_copy_c_string(fallback);
    }
    const char *utf8 = error.localizedDescription.UTF8String;
    if (utf8 == NULL) {
        return vision_copy_c_string(fallback);
    }
    return vision_copy_c_string(utf8);
}

static CGRect vision_normalized_roi(float x, float y, float width, float height) {
    float origin_x = x;
    float origin_y = 1.0f - (y + height);
    float clamped_x = fmaxf(0.0f, fminf(origin_x, 1.0f));
    float clamped_y = fmaxf(0.0f, fminf(origin_y, 1.0f));
    float max_width = 1.0f - clamped_x;
    float max_height = 1.0f - clamped_y;
    float clamped_width = fmaxf(0.0f, fminf(width, max_width));
    float clamped_height = fmaxf(0.0f, fminf(height, max_height));
    return CGRectMake(clamped_x, clamped_y, clamped_width, clamped_height);
}

VisionResult vision_detect_text_regions(
    const uint8_t *data,
    size_t width,
    size_t height,
    size_t stride,
    float roi_x,
    float roi_y,
    float roi_width,
    float roi_height
) {
    VisionResult result = {0};

    if (data == NULL || width == 0 || height == 0 || stride < width) {
        result.error = vision_copy_c_string("invalid input frame for Vision detection");
        return result;
    }

    float frame_width_f = (float)width;
    float frame_height_f = (float)height;
    float roi_left_norm = fmaxf(0.0f, fminf(roi_x, 1.0f));
    float roi_right_norm = fmaxf(roi_left_norm, fminf(roi_x + roi_width, 1.0f));
    float roi_top_norm = fmaxf(0.0f, fminf(roi_y, 1.0f));
    float roi_bottom_norm_top = fmaxf(roi_top_norm, fminf(roi_y + roi_height, 1.0f));
    float roi_width_norm = roi_right_norm - roi_left_norm;
    float roi_height_norm = roi_bottom_norm_top - roi_top_norm;
    if (roi_width_norm <= 0.0f || roi_height_norm <= 0.0f) {
        result.error = vision_copy_c_string("region of interest for Vision detection is empty");
        return result;
    }

    float roi_left_px = roi_left_norm * frame_width_f;
    float roi_width_px = roi_width_norm * frame_width_f;
    float roi_height_px = roi_height_norm * frame_height_f;
    float roi_bottom_norm = 1.0f - roi_bottom_norm_top;

    @autoreleasepool {
        size_t buffer_size = stride * height;

        CGColorSpaceRef color_space = CGColorSpaceCreateDeviceGray();
        if (color_space == NULL) {
            result.error = vision_copy_c_string("failed to create grayscale color space");
            return result;
        }

        CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, data, buffer_size, NULL);
        if (provider == NULL) {
            CGColorSpaceRelease(color_space);
            result.error = vision_copy_c_string("failed to create data provider for frame buffer");
            return result;
        }

        CGBitmapInfo bitmap_info = (CGBitmapInfo)kCGImageAlphaNone | kCGBitmapByteOrderDefault;
        CGImageRef image = CGImageCreate(
            width,
            height,
            8,
            8,
            stride,
            color_space,
            bitmap_info,
            provider,
            NULL,
            false,
            kCGRenderingIntentDefault
        );

        CGDataProviderRelease(provider);
        CGColorSpaceRelease(color_space);

        if (image == NULL) {
            result.error = vision_copy_c_string("failed to build CoreGraphics image for frame buffer");
            return result;
        }

        CGRect roi = vision_normalized_roi(roi_x, roi_y, roi_width, roi_height);
        if (roi.size.width <= 0.0f || roi.size.height <= 0.0f) {
            CGImageRelease(image);
            result.error = vision_copy_c_string("region of interest for Vision detection is empty");
            return result;
        }

        VNDetectTextRectanglesRequest *request = [[VNDetectTextRectanglesRequest alloc] init];
        request.reportCharacterBoxes = NO;
        request.regionOfInterest = roi;

        NSError *error = nil;
        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
        BOOL success = [handler performRequests:@[request] error:&error];

        CGImageRelease(image);

        if (!success) {
            result.error = vision_copy_nsstring(error, "vision text detection request failed");
            return result;
        }

        NSArray<VNTextObservation *> *observations = request.results;
        NSUInteger count = observations.count;
        if (count == 0) {
            return result;
        }

        VisionRegion *regions = malloc(sizeof(VisionRegion) * count);
        if (regions == NULL) {
            result.error = vision_copy_c_string("failed to allocate region buffer for Vision detection");
            return result;
        }

        for (NSUInteger i = 0; i < count; i++) {
            VNTextObservation *observation = observations[i];
            CGRect box = observation.boundingBox;
            float normalized_origin_x = (float)box.origin.x;
            float normalized_origin_y = (float)box.origin.y;
            float normalized_width = (float)box.size.width;
            float normalized_height = (float)box.size.height;

            float obs_width = normalized_width * roi_width_px;
            float obs_height = normalized_height * roi_height_px;
            float obs_x = roi_left_px + normalized_origin_x * roi_width_px;
            float region_bottom_norm = roi_bottom_norm + normalized_origin_y * roi_height_norm;
            region_bottom_norm = fmaxf(0.0f, fminf(region_bottom_norm, 1.0f));
            float obs_bottom = region_bottom_norm * frame_height_f;
            float obs_y = frame_height_f - (obs_bottom + obs_height);

            VisionRegion region;
            region.x = obs_x;
            region.y = obs_y;
            region.width = obs_width;
            region.height = obs_height;
            region.confidence = (float)observation.confidence;
            regions[i] = region;
        }

        result.regions = regions;
        result.count = (size_t)count;
    }

    return result;
}

void vision_result_destroy(VisionResult result) {
    if (result.regions != NULL) {
        free(result.regions);
    }
    if (result.error != NULL) {
        free(result.error);
    }
}
