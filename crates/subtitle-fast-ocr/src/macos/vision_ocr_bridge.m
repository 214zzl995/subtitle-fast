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
} VisionOcrRect;

typedef struct {
    VisionOcrRect rect;
    float confidence;
    char *text;
} VisionOcrText;

typedef struct {
    VisionOcrText *texts;
    size_t count;
    char *error;
} VisionOcrResult;

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

static char *vision_copy_string_value(NSString *value) {
    if (value == nil) {
        return NULL;
    }
    const char *utf8 = value.UTF8String;
    if (utf8 == NULL) {
        return NULL;
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

static void vision_destroy_result(VisionOcrResult *result) {
    if (result == NULL) {
        return;
    }
    if (result->texts != NULL) {
        for (size_t i = 0; i < result->count; i++) {
            if (result->texts[i].text != NULL) {
                free(result->texts[i].text);
                result->texts[i].text = NULL;
            }
        }
        free(result->texts);
        result->texts = NULL;
    }
    if (result->error != NULL) {
        free(result->error);
        result->error = NULL;
    }
    result->count = 0;
}

static bool vision_append_text(
    VisionOcrText **texts_ptr,
    size_t *count_ptr,
    size_t *capacity_ptr,
    VisionOcrText text
) {
    if (texts_ptr == NULL || count_ptr == NULL || capacity_ptr == NULL) {
        return false;
    }

    size_t count = *count_ptr;
    size_t capacity = *capacity_ptr;
    VisionOcrText *texts = *texts_ptr;

    if (count == capacity) {
        size_t new_capacity = capacity == 0 ? 8 : capacity * 2;
        VisionOcrText *new_texts = realloc(texts, new_capacity * sizeof(VisionOcrText));
        if (new_texts == NULL) {
            return false;
        }
        texts = new_texts;
        capacity = new_capacity;
        *texts_ptr = texts;
        *capacity_ptr = capacity;
    }

    texts[count] = text;
    *count_ptr = count + 1;
    return true;
}

VisionOcrResult vision_recognize_text(
    const uint8_t *data,
    size_t width,
    size_t height,
    size_t stride,
    const VisionOcrRect *regions,
    size_t regions_count
) {
    VisionOcrResult result = {0};

    if (data == NULL || width == 0 || height == 0 || stride < width) {
        result.error = vision_copy_c_string("invalid input frame for Vision OCR");
        return result;
    }

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
            result.error =
                vision_copy_c_string("failed to build CoreGraphics image for frame buffer");
            return result;
        }

        VisionOcrText *texts = NULL;
        size_t count = 0;
        size_t capacity = 0;

        float frame_width_f = (float)width;
        float frame_height_f = (float)height;

        size_t effective_regions = regions_count;
        VisionOcrRect fallback_region = {
            .x = 0.0f,
            .y = 0.0f,
            .width = frame_width_f,
            .height = frame_height_f,
        };
        const VisionOcrRect *regions_ptr = regions;
        if (regions_ptr == NULL || effective_regions == 0) {
            regions_ptr = &fallback_region;
            effective_regions = 1;
        }

        for (size_t idx = 0; idx < effective_regions; idx++) {
            VisionOcrRect region = regions_ptr[idx];

            float region_width = fmaxf(0.0f, region.width);
            float region_height = fmaxf(0.0f, region.height);
            if (region_width <= 0.0f || region_height <= 0.0f) {
                continue;
            }

            float region_left = region.x;
            float region_top = region.y;
            float region_right = region_left + region_width;
            float region_bottom_top = region_top + region_height;

            float roi_left_norm = fmaxf(0.0f, fminf(region_left / frame_width_f, 1.0f));
            float roi_right_norm_top =
                fmaxf(roi_left_norm, fminf(region_right / frame_width_f, 1.0f));
            float roi_top_norm = fmaxf(0.0f, fminf(region_top / frame_height_f, 1.0f));
            float roi_bottom_norm_top =
                fmaxf(roi_top_norm, fminf(region_bottom_top / frame_height_f, 1.0f));
            float roi_width_norm = roi_right_norm_top - roi_left_norm;
            float roi_height_norm = roi_bottom_norm_top - roi_top_norm;

            if (roi_width_norm <= 0.0f || roi_height_norm <= 0.0f) {
                continue;
            }

            CGRect roi = vision_normalized_roi(
                roi_left_norm,
                roi_top_norm,
                roi_width_norm,
                roi_height_norm
            );
            if (roi.size.width <= 0.0f || roi.size.height <= 0.0f) {
                continue;
            }

            VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
            request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
            request.usesLanguageCorrection = YES;
            request.regionOfInterest = roi;

            NSError *error = nil;
            VNImageRequestHandler *handler =
                [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
            BOOL success = [handler performRequests:@[request] error:&error];

            if (!success) {
                vision_destroy_result(&(VisionOcrResult){ .texts = texts, .count = count });
                texts = NULL;
                count = 0;
                capacity = 0;
                result.error = vision_copy_nsstring(error, "vision text recognition request failed");
                CGImageRelease(image);
                return result;
            }

            NSArray<VNRecognizedTextObservation *> *observations = request.results;
            NSUInteger obs_count = observations.count;
            if (obs_count == 0) {
                continue;
            }

            float roi_width_px = roi_width_norm * frame_width_f;
            float roi_height_px = roi_height_norm * frame_height_f;
            float roi_left_px = roi_left_norm * frame_width_f;
            float roi_bottom_norm = 1.0f - roi_bottom_norm_top;

            for (NSUInteger obs_idx = 0; obs_idx < obs_count; obs_idx++) {
                VNRecognizedTextObservation *observation = observations[obs_idx];
                NSArray<VNRecognizedText *> *candidates = [observation topCandidates:1];
                VNRecognizedText *best = candidates.firstObject;
                if (best == nil) {
                    continue;
                }

                NSString *string = best.string;
                if (string == nil || string.length == 0) {
                    continue;
                }

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

                char *text = vision_copy_string_value(string);
                if (text == NULL) {
                    continue;
                }

                VisionOcrText entry;
                entry.rect.x = obs_x;
                entry.rect.y = obs_y;
                entry.rect.width = obs_width;
                entry.rect.height = obs_height;
                entry.confidence = (float)best.confidence;
                entry.text = text;

                if (!vision_append_text(&texts, &count, &capacity, entry)) {
                    free(text);
                    vision_destroy_result(&(VisionOcrResult){ .texts = texts, .count = count });
                    texts = NULL;
                    count = 0;
                    capacity = 0;
                    result.error =
                        vision_copy_c_string("failed to allocate OCR result buffer for Vision");
                    CGImageRelease(image);
                    return result;
                }
            }
        }

        CGImageRelease(image);
        result.texts = texts;
        result.count = count;
    }

    return result;
}

void vision_ocr_result_destroy(VisionOcrResult result) {
    vision_destroy_result(&result);
}
