// macOS Vision.framework OCR bridge (in-process MVP).
// Linked only into lumen-platform-macos.

#import <Foundation/Foundation.h>
#import <Vision/Vision.h>
#import <ImageIO/ImageIO.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stdlib.h>
#include <string.h>

int lumen_ocr_is_supported(void) {
    if (@available(macOS 10.15, *)) {
        return 1;
    }
    return 0;
}

// accurate: 1 = Accurate + no language correction; 0 = Fast + language correction
// Returns malloc'd UTF-8 string: text\n---\n{avg_confidence}  OR for boxes JSON array.
// Caller frees with free().
static CGImageRef lumen_cgimage_from_bytes(const uint8_t *data, int len) {
    if (!data || len <= 0) return NULL;
    NSData *ns = [NSData dataWithBytesNoCopy:(void *)data length:(NSUInteger)len freeWhenDone:NO];
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

char *lumen_ocr_recognize_text(const uint8_t *data, int len,
                               const char **langs, int lang_count,
                               int accurate) {
    @autoreleasepool {
        if (@available(macOS 10.15, *)) {
            CGImageRef cg = lumen_cgimage_from_bytes(data, len);
            if (!cg) return strdup("");

            VNImageRequestHandler *handler =
                [[VNImageRequestHandler alloc] initWithCGImage:cg options:@{}];
            if (!handler) {
                CGImageRelease(cg);
                return strdup("");
            }

            VNRecognizeTextRequest *req = [[VNRecognizeTextRequest alloc] init];
            if (accurate) {
                req.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
                req.usesLanguageCorrection = NO;
            } else {
                req.recognitionLevel = VNRequestTextRecognitionLevelFast;
                req.usesLanguageCorrection = YES;
            }
            req.recognitionLanguages = lumen_langs(langs, lang_count);

            NSError *err = nil;
            [handler performRequests:@[req] error:&err];
            if (err) {
                CGImageRelease(cg);
                return strdup("");
            }

            NSMutableString *out = [NSMutableString string];
            double confSum = 0;
            NSUInteger confN = 0;
            for (VNRecognizedTextObservation *obs in req.results) {
                VNRecognizedText *top = [[obs topCandidates:1] firstObject];
                if (!top) continue;
                if (out.length > 0) [out appendString:@"\n"];
                [out appendString:top.string ?: @""];
                confSum += top.confidence;
                confN++;
            }
            double avg = confN ? confSum / confN : 0.0;
            [out appendFormat:@"\n---\n%.6f", avg];
            char *ret = strdup(out.UTF8String ?: "");
            CGImageRelease(cg);
            return ret;
        }
        return strdup("");
    }
}

char *lumen_ocr_recognize_boxes_json(const uint8_t *data, int len,
                                     const char **langs, int lang_count) {
    @autoreleasepool {
        if (@available(macOS 10.15, *)) {
            CGImageRef cg = lumen_cgimage_from_bytes(data, len);
            if (!cg) return strdup("[]");

            VNImageRequestHandler *handler =
                [[VNImageRequestHandler alloc] initWithCGImage:cg options:@{}];
            if (!handler) {
                CGImageRelease(cg);
                return strdup("[]");
            }

            VNRecognizeTextRequest *req = [[VNRecognizeTextRequest alloc] init];
            req.recognitionLevel = VNRequestTextRecognitionLevelFast;
            req.usesLanguageCorrection = YES;
            req.recognitionLanguages = lumen_langs(langs, lang_count);

            NSError *err = nil;
            [handler performRequests:@[req] error:&err];
            if (err) {
                CGImageRelease(cg);
                return strdup("[]");
            }

            NSMutableArray *boxes = [NSMutableArray array];
            for (VNRecognizedTextObservation *obs in req.results) {
                VNRecognizedText *top = [[obs topCandidates:1] firstObject];
                if (!top) continue;
                CGRect b = obs.boundingBox;
                [boxes addObject:@{
                    @"x": @(b.origin.x),
                    @"y": @(b.origin.y),
                    @"w": @(b.size.width),
                    @"h": @(b.size.height),
                    @"text": top.string ?: @"",
                    @"confidence": @(top.confidence)
                }];
            }
            NSData *json = [NSJSONSerialization dataWithJSONObject:boxes options:0 error:nil];
            NSString *s = json ? [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding]
                               : @"[]";
            char *ret = strdup(s.UTF8String ?: "[]");
            CGImageRelease(cg);
            return ret;
        }
        return strdup("[]");
    }
}

void lumen_ocr_free(char *p) {
    if (p) free(p);
}
