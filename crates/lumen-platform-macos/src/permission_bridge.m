// Explicit macOS privacy authorization requests.

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>

enum {
    LUMEN_MIC_AUTH_NOT_DETERMINED = 0,
    LUMEN_MIC_AUTH_RESTRICTED = 1,
    LUMEN_MIC_AUTH_DENIED = 2,
    LUMEN_MIC_AUTH_AUTHORIZED = 3,
    LUMEN_MIC_AUTH_TIMEOUT = -1,
};

int lumen_microphone_authorization_status(void) {
    if (@available(macOS 10.14, *)) {
        return (int)[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    }
    return LUMEN_MIC_AUTH_RESTRICTED;
}

int lumen_request_microphone_access(void) {
    if (@available(macOS 10.14, *)) {
        AVAuthorizationStatus current =
            [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
        if (current != AVAuthorizationStatusNotDetermined) {
            return (int)current;
        }

        __block AVAuthorizationStatus resolved = AVAuthorizationStatusNotDetermined;
        dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                 completionHandler:^(BOOL granted) {
            resolved = granted
                ? AVAuthorizationStatusAuthorized
                : [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
            dispatch_semaphore_signal(semaphore);
        }];

        long timed_out = dispatch_semaphore_wait(
            semaphore,
            dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC));
        if (timed_out != 0) {
            return LUMEN_MIC_AUTH_TIMEOUT;
        }
        return (int)resolved;
    }
    return LUMEN_MIC_AUTH_RESTRICTED;
}
