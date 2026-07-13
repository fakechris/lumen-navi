#import <Foundation/Foundation.h>
#import <Security/Security.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static char *lumen_keychain_copy_error(OSStatus status) {
    CFStringRef message = SecCopyErrorMessageString(status, NULL);
    if (!message) return strdup("unknown keychain error");
    NSString *text = CFBridgingRelease(message);
    return strdup(text.UTF8String ?: "unknown keychain error");
}

static NSDictionary *lumen_keychain_query(NSString *service, NSString *account) {
    return @{
        (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: service,
        (__bridge id)kSecAttrAccount: account,
    };
}

static OSStatus lumen_keychain_read(NSString *service, NSString *account, uint8_t out_key[32]) {
    NSMutableDictionary *query = [lumen_keychain_query(service, account) mutableCopy];
    query[(__bridge id)kSecReturnData] = @YES;
    query[(__bridge id)kSecMatchLimit] = (__bridge id)kSecMatchLimitOne;
    CFTypeRef result = NULL;
    OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
    if (status != errSecSuccess) return status;
    NSData *data = CFBridgingRelease(result);
    if (data.length != 32) return errSecDecode;
    memcpy(out_key, data.bytes, 32);
    return errSecSuccess;
}

int lumen_context_keychain_get_or_create(const char *service_utf8,
                                         const char *account_utf8,
                                         uint8_t out_key[32],
                                         char **out_error) {
    if (out_error) *out_error = NULL;
    if (!service_utf8 || !account_utf8 || !out_key) return -1;
    @autoreleasepool {
        NSString *service = [NSString stringWithUTF8String:service_utf8];
        NSString *account = [NSString stringWithUTF8String:account_utf8];
        if (!service || !account) return -1;

        OSStatus status = lumen_keychain_read(service, account, out_key);
        if (status == errSecSuccess) return 0;
        if (status != errSecItemNotFound) {
            if (out_error) *out_error = lumen_keychain_copy_error(status);
            return (int)status;
        }

        uint8_t generated[32];
        status = SecRandomCopyBytes(kSecRandomDefault, sizeof(generated), generated);
        if (status != errSecSuccess) {
            if (out_error) *out_error = lumen_keychain_copy_error(status);
            return (int)status;
        }
        NSMutableDictionary *item = [lumen_keychain_query(service, account) mutableCopy];
        item[(__bridge id)kSecValueData] = [NSData dataWithBytes:generated length:sizeof(generated)];
        item[(__bridge id)kSecAttrAccessible] = (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
        status = SecItemAdd((__bridge CFDictionaryRef)item, NULL);
        memset(generated, 0, sizeof(generated));
        if (status == errSecDuplicateItem) {
            status = lumen_keychain_read(service, account, out_key);
        } else if (status == errSecSuccess) {
            status = lumen_keychain_read(service, account, out_key);
        }
        if (status != errSecSuccess) {
            if (out_error) *out_error = lumen_keychain_copy_error(status);
            return (int)status;
        }
        return 0;
    }
}

void lumen_context_keychain_free(char *value) {
    if (value) free(value);
}
