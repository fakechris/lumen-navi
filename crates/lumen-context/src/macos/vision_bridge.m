// macOS Vision.framework OCR bridge (in-process product path).

#import <Foundation/Foundation.h>
#import <Vision/Vision.h>
#import <ImageIO/ImageIO.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stdlib.h>
#include <string.h>

enum {
    LUMEN_OCR_OK = 0,
    LUMEN_OCR_EMPTY_INPUT = 1,
    LUMEN_OCR_DECODE_FAILED = 2,
    LUMEN_OCR_VISION_ERROR = 3,
    LUMEN_OCR_UNSUPPORTED = 4,
};

int lumen_ocr_is_supported(void) {
    if (@available(macOS 10.15, *)) {
        return 1;
    }
    return 0;
}

void lumen_ocr_free(char *p) {
    if (p) free(p);
}

static CGImageRef lumen_cgimage_from_bytes(const uint8_t *data, int len) {
    if (!data || len <= 0) return NULL;
    // Copy bytes — caller buffer may not outlive async paths.
    NSData *ns = [NSData dataWithBytes:data length:(NSUInteger)len];
    if (!ns) return NULL;
    CGImageSourceRef src = CGImageSourceCreateWithData((__bridge CFDataRef)ns, NULL);
    if (!src) return NULL;
    CGImageRef img = CGImageSourceCreateImageAtIndex(src, 0, NULL);
    CFRelease(src);
    return img;
}

static NSArray<NSString *> *lumen_langs(const char **langs, int lang_count) {
    NSMutableArray *arr = [NSMutableArray array];
    if (langs && lang_count > 0) {
        for (int i = 0; i < lang_count; i++) {
            if (langs[i] && langs[i][0]) {
                [arr addObject:[NSString stringWithUTF8String:langs[i]]];
            }
        }
    }
    if (arr.count == 0) {
        [arr addObject:@"zh-Hans"];
        [arr addObject:@"en-US"];
    }
    return arr;
}

static char *dup_ns(NSString *s) {
    if (!s) return strdup("");
    const char *u = s.UTF8String;
    return strdup(u ? u : "");
}

// out_text: text\n---\nconfidence  (caller frees)
// out_err: optional human error (caller frees)
int lumen_ocr_recognize_text(const uint8_t *data, int len,
                             const char **langs, int lang_count,
                             int accurate,
                             char **out_text,
                             char **out_err) {
    if (out_text) *out_text = NULL;
    if (out_err) *out_err = NULL;
    @autoreleasepool {
        if (@available(macOS 10.15, *)) {
            if (!data || len <= 0) {
                if (out_err) *out_err = strdup("empty image input");
                return LUMEN_OCR_EMPTY_INPUT;
            }
            CGImageRef cg = lumen_cgimage_from_bytes(data, len);
            if (!cg) {
                if (out_err) *out_err = strdup("image decode failed");
                return LUMEN_OCR_DECODE_FAILED;
            }

            size_t w = CGImageGetWidth(cg);
            size_t h = CGImageGetHeight(cg);
            if (w == 0 || h == 0) {
                CGImageRelease(cg);
                if (out_err) *out_err = strdup("decoded image has zero dimensions");
                return LUMEN_OCR_DECODE_FAILED;
            }

            VNImageRequestHandler *handler =
                [[VNImageRequestHandler alloc] initWithCGImage:cg options:@{}];
            if (!handler) {
                CGImageRelease(cg);
                if (out_err) *out_err = strdup("failed to create VNImageRequestHandler");
                return LUMEN_OCR_VISION_ERROR;
            }

            VNRecognizeTextRequest *req = [[VNRecognizeTextRequest alloc] init];
            if (accurate) {
                req.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
                req.usesLanguageCorrection = NO;
            } else {
                req.recognitionLevel = VNRequestTextRecognitionLevelFast;
                req.usesLanguageCorrection = YES;
            }
            // Prefer automatic language detection when available (macOS 13+)
            if (@available(macOS 13.0, *)) {
                req.automaticallyDetectsLanguage = YES;
            }
            req.recognitionLanguages = lumen_langs(langs, lang_count);

            NSError *err = nil;
            BOOL ok = [handler performRequests:@[req] error:&err];
            if (!ok || err) {
                NSString *msg = err.localizedDescription ?: @"vision performRequests failed";
                if (out_err) *out_err = dup_ns(msg);
                CGImageRelease(cg);
                return LUMEN_OCR_VISION_ERROR;
            }

            NSMutableString *out = [NSMutableString string];
            double confSum = 0;
            NSUInteger confN = 0;
            for (VNRecognizedTextObservation *obs in req.results) {
                VNRecognizedText *top = [[obs topCandidates:1] firstObject];
                if (!top) continue;
                NSString *t = top.string;
                if (!t.length) continue;
                if (out.length > 0) [out appendString:@"\n"];
                [out appendString:t];
                confSum += top.confidence;
                confN++;
            }
            double avg = confN ? confSum / confN : 0.0;
            [out appendFormat:@"\n---\n%.6f", avg];
            if (out_text) *out_text = dup_ns(out);
            CGImageRelease(cg);
            return LUMEN_OCR_OK;
        }
        if (out_err) *out_err = strdup("Vision OCR requires macOS 10.15+");
        return LUMEN_OCR_UNSUPPORTED;
    }
}

// out_json: JSON array of boxes
int lumen_ocr_recognize_boxes_json(const uint8_t *data, int len,
                                   const char **langs, int lang_count,
                                   char **out_json,
                                   char **out_err) {
    if (out_json) *out_json = NULL;
    if (out_err) *out_err = NULL;
    @autoreleasepool {
        if (@available(macOS 10.15, *)) {
            if (!data || len <= 0) {
                if (out_err) *out_err = strdup("empty image input");
                return LUMEN_OCR_EMPTY_INPUT;
            }
            CGImageRef cg = lumen_cgimage_from_bytes(data, len);
            if (!cg) {
                if (out_err) *out_err = strdup("image decode failed");
                return LUMEN_OCR_DECODE_FAILED;
            }

            VNImageRequestHandler *handler =
                [[VNImageRequestHandler alloc] initWithCGImage:cg options:@{}];
            if (!handler) {
                CGImageRelease(cg);
                if (out_err) *out_err = strdup("failed to create VNImageRequestHandler");
                return LUMEN_OCR_VISION_ERROR;
            }

            VNRecognizeTextRequest *req = [[VNRecognizeTextRequest alloc] init];
            req.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
            req.usesLanguageCorrection = NO;
            if (@available(macOS 13.0, *)) {
                req.automaticallyDetectsLanguage = YES;
            }
            req.recognitionLanguages = lumen_langs(langs, lang_count);

            NSError *err = nil;
            BOOL ok = [handler performRequests:@[req] error:&err];
            if (!ok || err) {
                NSString *msg = err.localizedDescription ?: @"vision performRequests failed";
                if (out_err) *out_err = dup_ns(msg);
                CGImageRelease(cg);
                return LUMEN_OCR_VISION_ERROR;
            }

            NSMutableArray *boxes = [NSMutableArray array];
            for (VNRecognizedTextObservation *obs in req.results) {
                VNRecognizedText *top = [[obs topCandidates:1] firstObject];
                if (!top || !top.string.length) continue;
                CGRect b = obs.boundingBox;
                [boxes addObject:@{
                    @"x": @(b.origin.x),
                    @"y": @(b.origin.y),
                    @"w": @(b.size.width),
                    @"h": @(b.size.height),
                    @"text": top.string,
                    @"confidence": @(top.confidence)
                }];
            }
            NSError *jerr = nil;
            NSData *json = [NSJSONSerialization dataWithJSONObject:boxes options:0 error:&jerr];
            if (!json) {
                if (out_err) *out_err = strdup("json encode boxes failed");
                CGImageRelease(cg);
                return LUMEN_OCR_VISION_ERROR;
            }
            NSString *s = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
            if (out_json) *out_json = dup_ns(s ?: @"[]");
            CGImageRelease(cg);
            return LUMEN_OCR_OK;
        }
        if (out_err) *out_err = strdup("Vision OCR requires macOS 10.15+");
        return LUMEN_OCR_UNSUPPORTED;
    }
}
