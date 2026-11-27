// Operate the Code tab's public macOS accessibility controls. Restrict every
// press to the web area whose URL identifies the exact native session.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

static id attribute(AXUIElementRef element, CFStringRef key) {
    CFTypeRef value = NULL;
    if (AXUIElementCopyAttributeValue(element, key, &value) != kAXErrorSuccess) return nil;
    return CFBridgingRelease(value);
}

static NSArray *matches(NSArray *roots, BOOL (^predicate)(AXUIElementRef)) {
    NSMutableArray *queue = [roots mutableCopy];
    NSMutableArray *found = [NSMutableArray array];
    for (NSUInteger i = 0; i < queue.count && i < 5000; i++) {
        AXUIElementRef element = (__bridge AXUIElementRef)queue[i];
        if (predicate(element)) [found addObject:queue[i]];
        NSArray *children = attribute(element, kAXChildrenAttribute);
        if (children) [queue addObjectsFromArray:children];
    }
    return found;
}

static BOOL roleIs(AXUIElementRef element, NSString *role) {
    return [attribute(element, kAXRoleAttribute) isEqual:role];
}

static BOOL hasTitle(AXUIElementRef element, NSArray *titles) {
    NSString *name = attribute(element, kAXTitleAttribute);
    return name && [titles containsObject:name];
}

static BOOL isBypassOption(AXUIElementRef element, NSArray *titles) {
    if (!roleIs(element, @"AXMenuItem")) return NO;
    NSString *name = attribute(element, kAXTitleAttribute);
    for (NSString *title in titles) {
        if ([name isEqual:title] || [name hasPrefix:[title stringByAppendingString:@" "]]) return YES;
    }
    return NO;
}

static id targetPage(AXUIElementRef app, NSString *path) {
    id window = attribute(app, kAXFocusedWindowAttribute);
    if (!window) return nil;
    NSArray *pages = matches(@[window], ^BOOL(AXUIElementRef element) {
        if (!roleIs(element, @"AXWebArea")) return NO;
        NSURL *url = attribute(element, kAXURLAttribute);
        return [url isKindOfClass:[NSURL class]] && [url.scheme isEqual:@"app"] &&
               [url.host isEqual:@"localhost"] && [url.path isEqual:path];
    });
    return pages.count == 1 ? pages.firstObject : nil;
}

// A successful press only means AX accepted the action. The Rust caller
// verifies persistence in Claude's own session record before sending.
static int pickMode(AXUIElementRef app, NSString *path) {
    NSArray *bypass = @[@"Bypass permissions", @"绕过权限", @"跳过权限", @"略過權限"];
    NSArray *modes = @[@"Accept edits", @"Ask permissions", @"Manual", @"Auto", @"Plan", @"Plan mode",
                       @"接受编辑", @"自动接受编辑", @"手动", @"自动", @"计划", @"接受編輯", @"手動", @"自動", @"計畫"];
    BOOL opened = NO, selected = NO, confirmed = NO;
    for (NSUInteger attempt = 0; attempt < 24; attempt++) {
        @autoreleasepool {
            id page = targetPage(app, path);
            if (!page) {
                if (opened) return 2;
            } else {
                NSArray *controls = matches(@[page], ^BOOL(AXUIElementRef e) {
                    return roleIs(e, @"AXPopUpButton") || roleIs(e, @"AXMenuItem") ||
                           roleIs(e, @"AXHeading") || roleIs(e, @"AXButton");
                });
                NSMutableArray *candidates = [NSMutableArray array];
                BOOL question = NO;
                for (id obj in controls) {
                    AXUIElementRef e = (__bridge AXUIElementRef)obj;
                    if (roleIs(e, @"AXPopUpButton") && hasTitle(e, bypass)) return 0;
                    if (roleIs(e, @"AXHeading") && hasTitle(e, @[@"Bypass all permissions?", @"绕过所有权限？", @"略過所有權限？"])) question = YES;
                }
                for (id obj in controls) {
                    AXUIElementRef e = (__bridge AXUIElementRef)obj;
                    if (![attribute(e, kAXEnabledAttribute) boolValue]) continue;
                    if ((!opened && roleIs(e, @"AXPopUpButton") && hasTitle(e, modes)) ||
                        (opened && !selected && isBypassOption(e, bypass)) ||
                        (selected && !confirmed && question && roleIs(e, @"AXButton") && hasTitle(e, bypass))) {
                        [candidates addObject:obj];
                    }
                }
                if (candidates.count > 1) return 3;
                if (candidates.count == 1) {
                    AXUIElementRef button = (__bridge AXUIElementRef)candidates.firstObject;
                    if (AXUIElementPerformAction(button, kAXPressAction) != kAXErrorSuccess) return 3;
                    if (!opened) opened = YES;
                    else if (!selected) selected = YES;
                    else confirmed = YES;
                }
            }
        }
        [NSThread sleepForTimeInterval:0.12];
    }
    return opened ? 3 : 2;
}

int clawkit_claude_select_bypass(const char *sessionId) {
    @autoreleasepool {
        if (!AXIsProcessTrusted()) return 1;
        NSArray *apps = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.anthropic.claudefordesktop"];
        if (apps.count != 1) return 2;
        AXUIElementRef app = AXUIElementCreateApplication([apps.firstObject processIdentifier]);
        AXUIElementSetMessagingTimeout(app, 0.5);
        int result = pickMode(app, [@"/epitaxy/" stringByAppendingString:[NSString stringWithUTF8String:sessionId]]);
        CFRelease(app);
        return result;
    }
}
