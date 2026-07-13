#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>

static char *LumenScreenCopyUtf8(NSString *value) {
    if (value == nil || value.UTF8String == NULL) return NULL;
    return strdup(value.UTF8String);
}

void lumen_context_screen_free(void *value) {
    if (value != NULL) free(value);
}

int lumen_context_capture_window_png(
    uint32_t windowID,
    double timeoutSeconds,
    uint8_t **outBytes,
    size_t *outLength,
    uint32_t *outWidth,
    uint32_t *outHeight,
    double *outScale,
    char **outError) {
    @autoreleasepool {
        if (outBytes != NULL) *outBytes = NULL;
        if (outLength != NULL) *outLength = 0;
        if (outWidth != NULL) *outWidth = 0;
        if (outHeight != NULL) *outHeight = 0;
        if (outScale != NULL) *outScale = 1.0;
        if (outError != NULL) *outError = NULL;

        if (@available(macOS 14.0, *)) {
            double timeout = MAX(0.1, MIN(timeoutSeconds, 10.0));
            dispatch_time_t deadline = dispatch_time(
                DISPATCH_TIME_NOW, (int64_t)(timeout * NSEC_PER_SEC));
            dispatch_semaphore_t contentSemaphore = dispatch_semaphore_create(0);
            __block SCWindow *targetWindow = nil;
            __block NSError *contentError = nil;
            [SCShareableContent
                getShareableContentExcludingDesktopWindows:YES
                onScreenWindowsOnly:YES
                completionHandler:^(SCShareableContent *content, NSError *error) {
                    contentError = error;
                    for (SCWindow *window in content.windows) {
                        if (window.windowID == windowID) {
                            targetWindow = window;
                            break;
                        }
                    }
                    dispatch_semaphore_signal(contentSemaphore);
                }];
            if (dispatch_semaphore_wait(contentSemaphore, deadline) != 0) {
                if (outError != NULL) *outError = LumenScreenCopyUtf8(@"ScreenCaptureKit content lookup timed out");
                return 1;
            }
            if (targetWindow == nil) {
                NSString *message = contentError.localizedDescription ?: @"window is not shareable";
                if (outError != NULL) *outError = LumenScreenCopyUtf8(message);
                return 2;
            }

            SCContentFilter *filter = [[SCContentFilter alloc]
                initWithDesktopIndependentWindow:targetWindow];
            SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
            double scale = MAX(1.0, filter.pointPixelScale);
            configuration.width = (size_t)MAX(1.0, ceil(targetWindow.frame.size.width * scale));
            configuration.height = (size_t)MAX(1.0, ceil(targetWindow.frame.size.height * scale));
            configuration.showsCursor = NO;
            configuration.ignoreShadowsSingleWindow = YES;

            dispatch_semaphore_t captureSemaphore = dispatch_semaphore_create(0);
            __block CGImageRef capturedImage = NULL;
            __block NSError *captureError = nil;
            [SCScreenshotManager
                captureImageWithFilter:filter
                configuration:configuration
                completionHandler:^(CGImageRef image, NSError *error) {
                    captureError = error;
                    if (image != NULL) capturedImage = CGImageRetain(image);
                    dispatch_semaphore_signal(captureSemaphore);
                }];
            if (dispatch_semaphore_wait(captureSemaphore, deadline) != 0) {
                if (outError != NULL) *outError = LumenScreenCopyUtf8(@"ScreenCaptureKit window capture timed out");
                return 3;
            }
            if (capturedImage == NULL) {
                NSString *message = captureError.localizedDescription ?: @"window capture returned no image";
                if (outError != NULL) *outError = LumenScreenCopyUtf8(message);
                return 4;
            }

            NSMutableData *png = [NSMutableData data];
            CGImageDestinationRef destination = CGImageDestinationCreateWithData(
                (__bridge CFMutableDataRef)png, CFSTR("public.png"), 1, NULL);
            if (destination == NULL) {
                CGImageRelease(capturedImage);
                if (outError != NULL) *outError = LumenScreenCopyUtf8(@"failed to create PNG destination");
                return 5;
            }
            CGImageDestinationAddImage(destination, capturedImage, NULL);
            BOOL finalized = CGImageDestinationFinalize(destination);
            CFRelease(destination);
            size_t width = CGImageGetWidth(capturedImage);
            size_t height = CGImageGetHeight(capturedImage);
            CGImageRelease(capturedImage);
            if (!finalized || png.length == 0) {
                if (outError != NULL) *outError = LumenScreenCopyUtf8(@"failed to encode window PNG");
                return 6;
            }
            uint8_t *copy = malloc(png.length);
            if (copy == NULL) {
                if (outError != NULL) *outError = LumenScreenCopyUtf8(@"window PNG allocation failed");
                return 7;
            }
            memcpy(copy, png.bytes, png.length);
            if (outBytes != NULL) *outBytes = copy;
            if (outLength != NULL) *outLength = png.length;
            if (outWidth != NULL) *outWidth = (uint32_t)MIN(width, UINT32_MAX);
            if (outHeight != NULL) *outHeight = (uint32_t)MIN(height, UINT32_MAX);
            if (outScale != NULL) *outScale = scale;
            return 0;
        }
        if (outError != NULL) *outError = LumenScreenCopyUtf8(@"ScreenCaptureKit screenshots require macOS 14 or later");
        return 8;
    }
}
