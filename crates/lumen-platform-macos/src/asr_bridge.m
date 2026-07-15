// macOS Speech.framework ASR bridge (Observe enrichment — not dictation UI).

#import <Foundation/Foundation.h>
#import <Speech/Speech.h>
#include <stdlib.h>
#include <string.h>

enum {
    LUMEN_ASR_OK = 0,
    LUMEN_ASR_EMPTY = 1,
    LUMEN_ASR_AUTH = 2,
    LUMEN_ASR_UNAVAILABLE = 3,
    LUMEN_ASR_ERROR = 4,
    LUMEN_ASR_UNSUPPORTED = 5,
};

int lumen_asr_is_supported(void) {
    if (@available(macOS 10.15, *)) {
        return 1;
    }
    return 0;
}

void lumen_asr_free(char *p) {
    if (p) free(p);
}

static char *dup_ns(NSString *s) {
    if (!s) return strdup("");
    const char *u = s.UTF8String;
    return strdup(u ? u : "");
}

/// Transcribe a local audio file (WAV/M4A/etc. supported by AVFoundation).
/// Blocks until recognition finishes or errors (uses semaphore).
int lumen_asr_transcribe_file(
    const char *path_utf8,
    const char *locale_utf8,
    char **out_text,
    char **out_err
) {
    if (out_text) *out_text = NULL;
    if (out_err) *out_err = NULL;

    if (!lumen_asr_is_supported()) {
        if (out_err) *out_err = strdup("Speech framework requires macOS 10.15+");
        return LUMEN_ASR_UNSUPPORTED;
    }
    if (!path_utf8 || !path_utf8[0]) {
        if (out_err) *out_err = strdup("empty path");
        return LUMEN_ASR_EMPTY;
    }

    @autoreleasepool {
        NSString *path = [NSString stringWithUTF8String:path_utf8];
        NSURL *url = [NSURL fileURLWithPath:path];
        if (![[NSFileManager defaultManager] fileExistsAtPath:path]) {
            if (out_err) *out_err = strdup("audio file missing");
            return LUMEN_ASR_EMPTY;
        }

        NSString *locId = (locale_utf8 && locale_utf8[0])
            ? [NSString stringWithUTF8String:locale_utf8]
            : @"zh-CN";
        NSLocale *locale = [NSLocale localeWithLocaleIdentifier:locId];
        SFSpeechRecognizer *recognizer = [[SFSpeechRecognizer alloc] initWithLocale:locale];
        if (!recognizer) {
            // Fallback en-US
            recognizer = [[SFSpeechRecognizer alloc] initWithLocale:[NSLocale localeWithLocaleIdentifier:@"en-US"]];
        }
        if (!recognizer || !recognizer.isAvailable) {
            if (out_err) *out_err = strdup("speech recognizer unavailable for locale");
            return LUMEN_ASR_UNAVAILABLE;
        }

        // Authorization (may prompt once).
        __block SFSpeechRecognizerAuthorizationStatus auth = [SFSpeechRecognizer authorizationStatus];
        if (auth == SFSpeechRecognizerAuthorizationStatusNotDetermined) {
            dispatch_semaphore_t authSem = dispatch_semaphore_create(0);
            [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
                auth = status;
                dispatch_semaphore_signal(authSem);
            }];
            dispatch_semaphore_wait(authSem, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC));
        }
        if (auth != SFSpeechRecognizerAuthorizationStatusAuthorized) {
            if (out_err) *out_err = strdup("speech recognition not authorized");
            return LUMEN_ASR_AUTH;
        }

        SFSpeechURLRecognitionRequest *req =
            [[SFSpeechURLRecognitionRequest alloc] initWithURL:url];
        if (!req) {
            if (out_err) *out_err = strdup("failed to build recognition request");
            return LUMEN_ASR_ERROR;
        }
        req.shouldReportPartialResults = NO;
        if (@available(macOS 13.0, *)) {
            // Prefer on-device when available.
            req.requiresOnDeviceRecognition = NO;
        }

        __block NSString *finalText = nil;
        __block NSError *finalErr = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);

        [recognizer recognitionTaskWithRequest:req
                                 resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
            if (error) {
                finalErr = error;
                dispatch_semaphore_signal(sem);
                return;
            }
            if (result.isFinal) {
                finalText = result.bestTranscription.formattedString ?: @"";
                dispatch_semaphore_signal(sem);
            }
        }];

        // Cap wait at 120s for long audio.
        long wait = dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 120 * NSEC_PER_SEC));
        if (wait != 0) {
            if (out_err) *out_err = strdup("speech recognition timed out");
            return LUMEN_ASR_ERROR;
        }
        if (finalErr) {
            if (out_err) *out_err = dup_ns(finalErr.localizedDescription ?: @"recognition failed");
            return LUMEN_ASR_ERROR;
        }
        if (out_text) *out_text = dup_ns(finalText ?: @"");
        return LUMEN_ASR_OK;
    }
}
