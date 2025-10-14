#import <Foundation/Foundation.h>

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    float x;
    float y;
    float width;
    float height;
} MlxRect;

typedef struct {
    MlxRect rect;
    float confidence;
    char *text;
} MlxText;

typedef struct {
    MlxText *texts;
    size_t count;
    char *error;
} MlxResult;

typedef struct {
    char *model_path;
} MlxContext;

static __thread char *mlx_last_error = NULL;

static void mlx_clear_error(void) {
    if (mlx_last_error != NULL) {
        free(mlx_last_error);
        mlx_last_error = NULL;
    }
}

static void mlx_set_error(const char *message) {
    mlx_clear_error();
    if (message == NULL) {
        return;
    }
    size_t len = strlen(message);
    mlx_last_error = malloc(len + 1);
    if (mlx_last_error == NULL) {
        return;
    }
    memcpy(mlx_last_error, message, len);
    mlx_last_error[len] = '\0';
}

static char *mlx_copy_cstring(const char *value) {
    if (value == NULL) {
        return NULL;
    }
    size_t len = strlen(value);
    char *copy = malloc(len + 1);
    if (copy == NULL) {
        return NULL;
    }
    memcpy(copy, value, len);
    copy[len] = '\0';
    return copy;
}

MlxContext *mlx_vlm_create(const char *model_path) {
    mlx_clear_error();
    if (model_path == NULL) {
        mlx_set_error("mlx_vlm model path is null");
        return NULL;
    }
    MlxContext *ctx = malloc(sizeof(MlxContext));
    if (ctx == NULL) {
        mlx_set_error("failed to allocate mlx_vlm context");
        return NULL;
    }
    ctx->model_path = mlx_copy_cstring(model_path);
    if (ctx->model_path == NULL) {
        free(ctx);
        mlx_set_error("failed to copy mlx_vlm model path");
        return NULL;
    }
    return ctx;
}

void mlx_vlm_destroy(MlxContext *ctx) {
    if (ctx == NULL) {
        return;
    }
    if (ctx->model_path != NULL) {
        free(ctx->model_path);
        ctx->model_path = NULL;
    }
    free(ctx);
}

const char *mlx_vlm_last_error(void) {
    return mlx_last_error;
}

static void mlx_result_append_text(MlxResult *result, MlxText text) {
    if (result->texts == NULL) {
        result->texts = malloc(sizeof(MlxText));
        if (result->texts == NULL) {
            return;
        }
        result->count = 0;
    } else {
        MlxText *resized = realloc(result->texts, sizeof(MlxText) * (result->count + 1));
        if (resized == NULL) {
            return;
        }
        result->texts = resized;
    }
    result->texts[result->count] = text;
    result->count += 1;
}

MlxResult mlx_vlm_recognize(
    MlxContext *ctx,
    const uint8_t *data,
    size_t width,
    size_t height,
    size_t stride,
    const MlxRect *regions,
    size_t regions_count
) {
    (void)data;
    (void)stride;
    MlxResult result = {0};
    mlx_clear_error();

    if (ctx == NULL) {
        result.error = mlx_copy_cstring("mlx_vlm context is null");
        return result;
    }
    if (regions == NULL || regions_count == 0) {
        return result;
    }
    if (width == 0 || height == 0) {
        result.error = mlx_copy_cstring("invalid frame dimensions for mlx_vlm");
        return result;
    }

    for (size_t idx = 0; idx < regions_count; idx++) {
        const MlxRect region = regions[idx];
        if (region.width <= 0.0f || region.height <= 0.0f) {
            continue;
        }

        char buffer[128];
        int written = snprintf(
            buffer,
            sizeof(buffer),
            "mlx_vlm[%zu] model=%s",
            idx,
            ctx->model_path != NULL ? ctx->model_path : "<none>"
        );
        if (written < 0) {
            continue;
        }
        size_t len = (size_t)written;
        char *text = malloc(len + 1);
        if (text == NULL) {
            continue;
        }
        memcpy(text, buffer, len);
        text[len] = '\0';

        MlxText entry;
        entry.rect = region;
        entry.confidence = 0.5f;
        entry.text = text;
        mlx_result_append_text(&result, entry);
    }

    return result;
}

void mlx_vlm_result_destroy(MlxResult result) {
    if (result.texts != NULL) {
        for (size_t idx = 0; idx < result.count; idx++) {
            if (result.texts[idx].text != NULL) {
                free(result.texts[idx].text);
                result.texts[idx].text = NULL;
            }
        }
        free(result.texts);
    }
    if (result.error != NULL) {
        free(result.error);
    }
}
