#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#include <stdlib.h>
#include <string.h>

static char *LumenCopyUtf8(NSString *value) {
    if (value == nil) return NULL;
    const char *utf8 = value.UTF8String;
    if (utf8 == NULL) return NULL;
    return strdup(utf8);
}

void lumen_context_ax_free(char *value) {
    if (value != NULL) free(value);
}

static id LumenAXCopy(AXUIElementRef element, CFStringRef attribute) {
    if (element == NULL) return nil;
    CFTypeRef value = NULL;
    AXError error = AXUIElementCopyAttributeValue(element, attribute, &value);
    if (error != kAXErrorSuccess || value == NULL) return nil;
    return CFBridgingRelease(value);
}

static NSString *LumenString(id value) {
    if ([value isKindOfClass:[NSString class]]) return value;
    if ([value isKindOfClass:[NSAttributedString class]]) {
        return [(NSAttributedString *)value string];
    }
    if ([value isKindOfClass:[NSNumber class]]) return [value stringValue];
    return nil;
}

static NSString *LumenAXString(AXUIElementRef element, CFStringRef attribute) {
    return LumenString(LumenAXCopy(element, attribute));
}

static NSNumber *LumenAXBool(AXUIElementRef element, CFStringRef attribute) {
    id value = LumenAXCopy(element, attribute);
    if ([value isKindOfClass:[NSNumber class]]) {
        return [NSNumber numberWithBool:[value boolValue]];
    }
    return nil;
}

static NSDictionary *LumenRect(AXUIElementRef element) {
    id positionValue = LumenAXCopy(element, kAXPositionAttribute);
    id sizeValue = LumenAXCopy(element, kAXSizeAttribute);
    if (positionValue == nil || sizeValue == nil) return nil;
    CGPoint position = CGPointZero;
    CGSize size = CGSizeZero;
    if (!AXValueGetValue((__bridge AXValueRef)positionValue, kAXValueCGPointType, &position)) {
        return nil;
    }
    if (!AXValueGetValue((__bridge AXValueRef)sizeValue, kAXValueCGSizeType, &size)) {
        return nil;
    }
    return @{
        @"x": @(position.x),
        @"y": @(position.y),
        @"width": @(size.width),
        @"height": @(size.height),
    };
}

static NSDictionary *LumenRange(AXUIElementRef element) {
    id rangeValue = LumenAXCopy(element, kAXSelectedTextRangeAttribute);
    if (rangeValue == nil) return nil;
    CFRange range = CFRangeMake(0, 0);
    if (!AXValueGetValue((__bridge AXValueRef)rangeValue, kAXValueCFRangeType, &range)) {
        return nil;
    }
    if (range.location < 0 || range.length < 0) return nil;
    return @{ @"location": @(range.location), @"length": @(range.length) };
}

static NSString *LumenTruncate(NSString *value, NSUInteger maxChars) {
    if (value == nil) return nil;
    if (value.length <= maxChars) return value;
    return [[value substringToIndex:maxChars] stringByAppendingString:@"…[truncated]"];
}

static NSString *LumenIsoDate(NSDate *date) {
    if (date == nil) return nil;
    static NSISO8601DateFormatter *formatter;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        formatter = [[NSISO8601DateFormatter alloc] init];
        formatter.formatOptions = NSISO8601DateFormatWithInternetDateTime |
                                  NSISO8601DateFormatWithFractionalSeconds;
    });
    return [formatter stringFromDate:date];
}

static void LumenPut(NSMutableDictionary *dictionary, NSString *key, id value) {
    if (value != nil) dictionary[key] = value;
}

static NSDictionary *LumenWindowInfo(pid_t pid) {
    CFArrayRef copied = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID);
    if (copied == NULL) return nil;
    NSArray *windows = CFBridgingRelease(copied);
    for (NSDictionary *window in windows) {
        if ([window[(id)kCGWindowOwnerPID] intValue] != pid) continue;
        if ([window[(id)kCGWindowLayer] intValue] != 0) continue;
        return window;
    }
    return nil;
}

static BOOL LumenIsSecure(NSString *role, NSString *subrole) {
    NSString *joined = [NSString stringWithFormat:@"%@ %@", role ?: @"", subrole ?: @""];
    return [joined rangeOfString:@"secure" options:NSCaseInsensitiveSearch].location != NSNotFound;
}

static NSString *LumenNeighborText(AXUIElementRef focused, NSInteger offset, NSUInteger maxChars) {
    id parentValue = LumenAXCopy(focused, kAXParentAttribute);
    if (parentValue == nil) return nil;
    AXUIElementRef parent = (__bridge AXUIElementRef)parentValue;
    id childrenValue = LumenAXCopy(parent, kAXChildrenAttribute);
    if (![childrenValue isKindOfClass:[NSArray class]]) return nil;
    NSArray *children = childrenValue;
    NSInteger current = NSNotFound;
    for (NSUInteger index = 0; index < children.count; index++) {
        if (CFEqual((__bridge CFTypeRef)children[index], focused)) {
            current = (NSInteger)index;
            break;
        }
    }
    if (current == NSNotFound) return nil;
    NSInteger neighborIndex = current + offset;
    if (neighborIndex < 0 || neighborIndex >= (NSInteger)children.count) return nil;
    AXUIElementRef neighbor = (__bridge AXUIElementRef)children[(NSUInteger)neighborIndex];
    NSString *text = LumenAXString(neighbor, kAXTitleAttribute);
    if (text.length == 0) text = LumenAXString(neighbor, kAXValueAttribute);
    if (text.length == 0) text = LumenAXString(neighbor, kAXDescriptionAttribute);
    return LumenTruncate(text, maxChars);
}

static NSArray *LumenAncestorPath(AXUIElementRef focused) {
    NSMutableArray *path = [NSMutableArray array];
    NSMutableSet<NSValue *> *visited = [NSMutableSet set];
    id currentValue = (__bridge id)focused;
    for (NSUInteger depth = 0; depth < 16 && currentValue != nil; depth++) {
        AXUIElementRef current = (__bridge AXUIElementRef)currentValue;
        NSValue *identity = [NSValue valueWithPointer:current];
        if ([visited containsObject:identity]) break;
        [visited addObject:identity];
        NSString *role = LumenAXString(current, kAXRoleAttribute) ?: @"unknown";
        NSString *title = LumenAXString(current, kAXTitleAttribute);
        [path addObject:title.length > 0
            ? [NSString stringWithFormat:@"%@:%@", role, title]
            : role];
        id parentValue = LumenAXCopy(current, kAXParentAttribute);
        if (parentValue == nil) break;
        currentValue = parentValue;
    }
    return path;
}

int lumen_context_capture_ax_fast(
    uint64_t maxCharsRaw,
    double timeoutSeconds,
    char **outJson,
    char **outError) {
    @autoreleasepool {
        if (outJson != NULL) *outJson = NULL;
        if (outError != NULL) *outError = NULL;
        NSUInteger maxChars = (NSUInteger)MIN(maxCharsRaw, (uint64_t)NSUIntegerMax);

        NSRunningApplication *running = NSWorkspace.sharedWorkspace.frontmostApplication;
        if (running == nil) {
            if (outError != NULL) *outError = LumenCopyUtf8(@"frontmost application unavailable");
            return 1;
        }

        pid_t pid = running.processIdentifier;
        NSDate *capturedAt = [NSDate date];
        NSMutableDictionary *target = [NSMutableDictionary dictionary];
        LumenPut(target, @"app_name", running.localizedName);
        LumenPut(target, @"bundle_id", running.bundleIdentifier);
        target[@"pid"] = @(pid);
        LumenPut(target, @"process_start_time", LumenIsoDate(running.launchDate));
        LumenPut(target, @"captured_at", LumenIsoDate(capturedAt));

        NSDictionary *windowInfo = LumenWindowInfo(pid);
        if (windowInfo != nil) {
            LumenPut(target, @"window_id", windowInfo[(id)kCGWindowNumber]);
            LumenPut(target, @"window_title", windowInfo[(id)kCGWindowName]);
        }

        BOOL trusted = AXIsProcessTrusted();
        NSMutableDictionary *envelope = [NSMutableDictionary dictionaryWithDictionary:@{
            @"target": target,
            @"accessibility_trusted": [NSNumber numberWithBool:trusted],
        }];

        if (trusted) {
            AXUIElementRef app = AXUIElementCreateApplication(pid);
            AXUIElementSetMessagingTimeout(app, MAX(0.05, MIN(timeoutSeconds, 5.0)));
            id windowValue = LumenAXCopy(app, kAXFocusedWindowAttribute);
            id focusedValue = LumenAXCopy(app, kAXFocusedUIElementAttribute);
            AXUIElementRef window = (__bridge AXUIElementRef)windowValue;
            AXUIElementRef focused = (__bridge AXUIElementRef)focusedValue;

            if (window != NULL) {
                NSString *title = LumenAXString(window, kAXTitleAttribute);
                LumenPut(target, @"window_title", title);
                LumenPut(target, @"window_bounds_global", LumenRect(window));
            }

            if (focused != NULL) {
                NSString *role = LumenAXString(focused, kAXRoleAttribute);
                NSString *subrole = LumenAXString(focused, kAXSubroleAttribute);
                NSString *identifier = LumenAXString(focused, CFSTR("AXIdentifier"));
                NSDictionary *bounds = LumenRect(focused);
                BOOL secure = LumenIsSecure(role, subrole);
                NSMutableDictionary *editor = [NSMutableDictionary dictionary];
                LumenPut(editor, @"role", role);
                LumenPut(editor, @"subrole", subrole);
                LumenPut(editor, @"ax_identifier", identifier);
                LumenPut(editor, @"enabled", LumenAXBool(focused, kAXEnabledAttribute));
                LumenPut(editor, @"focused", LumenAXBool(focused, kAXFocusedAttribute));
                Boolean settable = false;
                AXUIElementIsAttributeSettable(focused, kAXValueAttribute, &settable);
                editor[@"editable"] = [NSNumber numberWithBool:(settable != false)];
                editor[@"secure"] = [NSNumber numberWithBool:secure];
                LumenPut(editor, @"bounds_global", bounds);
                LumenPut(editor, @"captured_at", LumenIsoDate(capturedAt));
                editor[@"truncated"] = [NSNumber numberWithBool:NO];

                NSDictionary *range = LumenRange(focused);
                if (!secure) {
                    LumenPut(editor, @"title", LumenAXString(focused, kAXTitleAttribute));
                    LumenPut(editor, @"label", LumenAXString(focused, kAXDescriptionAttribute));
                    LumenPut(editor, @"placeholder", LumenAXString(focused, CFSTR("AXPlaceholderValue")));
                    editor[@"ancestor_path"] = LumenAncestorPath(focused);
                    LumenPut(editor, @"nearby_before", LumenNeighborText(focused, -1, maxChars));
                    LumenPut(editor, @"nearby_after", LumenNeighborText(focused, 1, maxChars));
                    NSString *value = LumenAXString(focused, kAXValueAttribute);
                    NSString *selected = LumenAXString(focused, kAXSelectedTextAttribute);
                    if (value != nil) {
                        editor[@"value_length"] = @(value.length);
                        BOOL truncated = value.length > maxChars;
                        editor[@"truncated"] = [NSNumber numberWithBool:truncated];
                        LumenPut(editor, @"full_field_text", LumenTruncate(value, maxChars));
                    }
                    LumenPut(editor, @"selected_text", LumenTruncate(selected, maxChars));
                    LumenPut(editor, @"selection_range", range);
                    if (value != nil && range != nil) {
                        NSUInteger location = [range[@"location"] unsignedIntegerValue];
                        NSUInteger length = [range[@"length"] unsignedIntegerValue];
                        if (location <= value.length && location + length <= value.length) {
                            LumenPut(editor, @"cursor_prefix",
                                     LumenTruncate([value substringToIndex:location], maxChars));
                            LumenPut(editor, @"cursor_suffix",
                                     LumenTruncate([value substringFromIndex:location + length], maxChars));
                        }
                    }
                } else {
                    editor[@"ancestor_path"] = @[];
                    [target removeObjectForKey:@"window_title"];
                    [target removeObjectForKey:@"window_bounds_global"];
                }

                envelope[@"editor"] = editor;
                NSString *material = [NSString stringWithFormat:@"%d|%@|%@|%@|%@|%@",
                    pid,
                    target[@"window_id"] ?: @"",
                    role ?: @"",
                    subrole ?: @"",
                    identifier ?: @"",
                    bounds.description ?: @""];
                envelope[@"fingerprint_material"] = material;
            }
            CFRelease(app);
        }

        NSError *jsonError = nil;
        NSData *data = [NSJSONSerialization dataWithJSONObject:envelope options:0 error:&jsonError];
        if (data == nil) {
            if (outError != NULL) *outError = LumenCopyUtf8(jsonError.localizedDescription);
            return 2;
        }
        NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
        if (outJson != NULL) *outJson = LumenCopyUtf8(json);
        return 0;
    }
}

typedef struct {
    NSUInteger maxNodes;
    NSUInteger maxDepth;
    NSUInteger maxChars;
    NSUInteger visitedNodes;
    NSUInteger usedChars;
    NSUInteger truncatedNodes;
    NSUInteger truncatedChars;
    CFAbsoluteTime deadline;
} LumenAXTraversal;

static NSString *LumenBudgetString(NSString *value, LumenAXTraversal *state, BOOL secure) {
    if (value == nil || secure) return nil;
    if (state->usedChars >= state->maxChars) {
        state->truncatedChars += value.length;
        return nil;
    }
    NSUInteger remaining = state->maxChars - state->usedChars;
    if (value.length > remaining) {
        NSString *prefix = [value substringToIndex:remaining];
        state->usedChars += remaining;
        state->truncatedChars += value.length - remaining;
        return prefix;
    }
    state->usedChars += value.length;
    return value;
}

static NSDictionary *LumenAXNode(
    AXUIElementRef element,
    NSString *path,
    NSUInteger depth,
    NSUInteger siblingIndex,
    LumenAXTraversal *state,
    NSMutableSet<NSValue *> *visited) {
    if (element == NULL) return nil;
    if (depth >= state->maxDepth) {
        state->truncatedNodes += 1;
        return nil;
    }
    if (state->visitedNodes >= state->maxNodes || CFAbsoluteTimeGetCurrent() >= state->deadline) {
        state->truncatedNodes += 1;
        return nil;
    }
    NSValue *identity = [NSValue valueWithPointer:element];
    if ([visited containsObject:identity]) return nil;
    [visited addObject:identity];
    state->visitedNodes += 1;

    NSString *role = LumenAXString(element, kAXRoleAttribute) ?: @"unknown";
    NSString *subrole = LumenAXString(element, kAXSubroleAttribute);
    BOOL secure = LumenIsSecure(role, subrole);
    NSMutableDictionary *node = [NSMutableDictionary dictionary];
    node[@"stable_path"] = path;
    node[@"role"] = role;
    LumenPut(node, @"subrole", subrole);
    LumenPut(node, @"title", LumenBudgetString(
        LumenAXString(element, kAXTitleAttribute), state, secure));
    LumenPut(node, @"value", LumenBudgetString(
        LumenAXString(element, kAXValueAttribute), state, secure));
    LumenPut(node, @"description", LumenBudgetString(
        LumenAXString(element, kAXDescriptionAttribute), state, secure));
    LumenPut(node, @"placeholder", LumenBudgetString(
        LumenAXString(element, CFSTR("AXPlaceholderValue")), state, secure));
    NSDictionary *bounds = LumenRect(element);
    LumenPut(node, @"bounds_global", bounds);
    LumenPut(node, @"enabled", LumenAXBool(element, kAXEnabledAttribute));
    LumenPut(node, @"focused", LumenAXBool(element, kAXFocusedAttribute));
    LumenPut(node, @"selected", LumenAXBool(element, kAXSelectedAttribute));
    node[@"depth"] = @(depth);
    node[@"sibling_index"] = @(siblingIndex);
    if (bounds != nil) {
        BOOL visible = [bounds[@"width"] doubleValue] > 0.0 &&
                       [bounds[@"height"] doubleValue] > 0.0;
        node[@"visible_on_screen"] = [NSNumber numberWithBool:visible];
    }

    NSMutableArray *capturedChildren = [NSMutableArray array];
    if (!secure) {
        id childrenValue = LumenAXCopy(element, kAXChildrenAttribute);
        if ([childrenValue isKindOfClass:[NSArray class]]) {
            NSArray *children = childrenValue;
            for (NSUInteger index = 0; index < children.count; index++) {
                if (state->visitedNodes >= state->maxNodes ||
                    CFAbsoluteTimeGetCurrent() >= state->deadline) {
                    state->truncatedNodes += children.count - index;
                    break;
                }
                AXUIElementRef child = (__bridge AXUIElementRef)children[index];
                NSString *childRole = LumenAXString(child, kAXRoleAttribute) ?: @"unknown";
                NSString *childPath = [NSString stringWithFormat:@"%@/%@[%lu]",
                    path, childRole, (unsigned long)index];
                NSDictionary *captured = LumenAXNode(
                    child, childPath, depth + 1, index, state, visited);
                if (captured != nil) [capturedChildren addObject:captured];
            }
        }
    }
    node[@"children"] = capturedChildren;
    return node;
}

int lumen_context_capture_ax_visible(
    uint64_t maxNodesRaw,
    uint64_t maxDepthRaw,
    uint64_t maxCharsRaw,
    double timeoutSeconds,
    char **outJson,
    char **outError) {
    @autoreleasepool {
        if (outJson != NULL) *outJson = NULL;
        if (outError != NULL) *outError = NULL;
        if (!AXIsProcessTrusted()) {
            if (outError != NULL) {
                *outError = LumenCopyUtf8(@"Accessibility permission is not granted");
            }
            return 2;
        }
        NSRunningApplication *running = NSWorkspace.sharedWorkspace.frontmostApplication;
        if (running == nil) {
            if (outError != NULL) *outError = LumenCopyUtf8(@"frontmost application unavailable");
            return 1;
        }

        AXUIElementRef app = AXUIElementCreateApplication(running.processIdentifier);
        AXUIElementSetMessagingTimeout(app, MAX(0.05, MIN(timeoutSeconds, 5.0)));
        id windowValue = LumenAXCopy(app, kAXFocusedWindowAttribute);
        AXUIElementRef window = (__bridge AXUIElementRef)windowValue;
        LumenAXTraversal state = {
            .maxNodes = (NSUInteger)MIN(maxNodesRaw, (uint64_t)NSUIntegerMax),
            .maxDepth = (NSUInteger)MIN(maxDepthRaw, (uint64_t)NSUIntegerMax),
            .maxChars = (NSUInteger)MIN(maxCharsRaw, (uint64_t)NSUIntegerMax),
            .visitedNodes = 0,
            .usedChars = 0,
            .truncatedNodes = 0,
            .truncatedChars = 0,
            .deadline = CFAbsoluteTimeGetCurrent() + MAX(0.05, MIN(timeoutSeconds, 5.0)),
        };
        NSMutableArray *roots = [NSMutableArray array];
        if (window != NULL) {
            NSMutableSet *visited = [NSMutableSet set];
            NSDictionary *root = LumenAXNode(
                window, @"/AXWindow[0]", 0, 0, &state, visited);
            if (root != nil) [roots addObject:root];
        }
        CFRelease(app);

        NSDictionary *result = @{
            @"roots": roots,
            @"captured_at": LumenIsoDate([NSDate date]),
            @"visited_nodes": @(state.visitedNodes),
            @"hidden_nodes": @0,
            @"truncated_nodes": @(state.truncatedNodes),
            @"truncated_chars": @(state.truncatedChars),
        };
        NSError *jsonError = nil;
        NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:&jsonError];
        if (data == nil) {
            if (outError != NULL) *outError = LumenCopyUtf8(jsonError.localizedDescription);
            return 3;
        }
        NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
        if (outJson != NULL) *outJson = LumenCopyUtf8(json);
        return 0;
    }
}
