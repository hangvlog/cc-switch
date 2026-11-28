// Operate the Code tab's public macOS accessibility controls. Restrict every
// press to the web area whose URL identifies the exact native session, and
// snapshot/restore the user's active window, session, and composer focus around
// desktop navigation.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

static id attribute(AXUIElementRef element, CFStringRef key) {
    if (!element) return nil;
    CFTypeRef value = NULL;
    return AXUIElementCopyAttributeValue(element, key, &value) == kAXErrorSuccess
        ? CFBridgingRelease(value) : nil;
}

static BOOL roleIs(AXUIElementRef element, NSString *role) {
    return [attribute(element, kAXRoleAttribute) isEqual:role];
}

static NSArray *matches(NSArray *roots, BOOL (^predicate)(AXUIElementRef)) {
    NSMutableArray *queue = [roots mutableCopy];
    NSMutableArray *found = [NSMutableArray array];
    for (NSUInteger i = 0; i < queue.count && i < 6000; i++) {
        AXUIElementRef element = (__bridge AXUIElementRef)queue[i];
        NSString *role = attribute(element, kAXRoleAttribute);
        if (i > 0 && ([role isEqual:@"AXApplication"] || [role isEqual:@"AXMenuBar"] || [role isEqual:@"AXMenuBarItem"])) {
            continue;
        }
        if (predicate(element)) [found addObject:queue[i]];
        NSArray *children = attribute(element, kAXChildrenAttribute);
        if (children) [queue addObjectsFromArray:children];
    }
    return found;
}

static BOOL hasTitle(AXUIElementRef element, NSArray *titles) {
    NSString *name = attribute(element, kAXTitleAttribute);
    return name && [titles containsObject:name];
}

static BOOL hasTitleOrDesc(AXUIElementRef element, NSArray *titles) {
    if (hasTitle(element, titles)) return YES;
    NSString *desc = attribute(element, kAXDescriptionAttribute);
    return desc && [titles containsObject:desc];
}

static BOOL isBypassOption(AXUIElementRef element, NSArray *titles) {
    if (!roleIs(element, @"AXMenuItem")) return NO;
    NSString *name = attribute(element, kAXTitleAttribute);
    for (NSString *title in titles) {
        if ([name isEqual:title] || [name hasPrefix:[title stringByAppendingString:@" "]]) return YES;
    }
    return NO;
}

static NSArray *claudeWindows(AXUIElementRef app) {
    NSArray *raw = attribute(app, kAXWindowsAttribute);
    NSMutableArray *wins = [NSMutableArray array];
    for (id obj in raw ?: @[]) {
        if (roleIs((__bridge AXUIElementRef)obj, @"AXWindow")) [wins addObject:obj];
    }
    return wins;
}

static id findWebArea(id window, BOOL (^urlPredicate)(NSURL *)) {
    if (!window) return nil;
    NSArray *pages = matches(@[window], ^BOOL(AXUIElementRef element) {
        if (!roleIs(element, @"AXWebArea")) return NO;
        NSURL *url = attribute(element, kAXURLAttribute);
        return [url isKindOfClass:[NSURL class]] && urlPredicate(url);
    });
    return pages.count == 1 ? pages.firstObject : nil;
}

static id targetPage(AXUIElementRef app, NSString *path) {
    BOOL (^matchPath)(NSURL *) = ^BOOL(NSURL *url) {
        return [url.scheme isEqual:@"app"] && [url.host isEqual:@"localhost"] && [url.path isEqual:path];
    };
    id focused = attribute(app, kAXFocusedWindowAttribute);
    id page = findWebArea(focused, matchPath);
    if (page) return page;
    for (id win in claudeWindows(app)) {
        if (focused && CFEqual((__bridge CFTypeRef)win, (__bridge CFTypeRef)focused)) continue;
        page = findWebArea(win, matchPath);
        if (page) return page;
    }
    return nil;
}

static id findMainAppWebArea(AXUIElementRef app, id *outWindow) {
    BOOL (^isAppPage)(NSURL *) = ^BOOL(NSURL *url) {
        return [url.scheme isEqual:@"app"] && [url.host isEqual:@"localhost"] && url.path.length > 0;
    };
    for (id win in claudeWindows(app)) {
        id page = findWebArea(win, isAppPage);
        if (page) {
            if (outWindow) *outWindow = win;
            return page;
        }
    }
    return nil;
}

static id findPromptField(id window) {
    if (!window) return nil;
    NSArray *fields = matches(@[window], ^BOOL(AXUIElementRef e) {
        return roleIs(e, @"AXTextArea") && hasTitleOrDesc(e, @[@"Prompt", @"提示"]);
    });
    if (fields.count == 1) return fields.firstObject;
    NSMutableArray *focused = [NSMutableArray array];
    for (id obj in fields) {
        if ([attribute((__bridge AXUIElementRef)obj, kAXFocusedAttribute) boolValue]) [focused addObject:obj];
    }
    return focused.count == 1 ? focused.firstObject : nil;
}

static BOOL readRange(AXUIElementRef field, CFRange *outRange) {
    id raw = attribute(field, kAXSelectedTextRangeAttribute);
    return raw && CFGetTypeID((__bridge CFTypeRef)raw) == AXValueGetTypeID() &&
           AXValueGetType((__bridge AXValueRef)raw) == kAXValueCFRangeType &&
           AXValueGetValue((__bridge AXValueRef)raw, kAXValueCFRangeType, outRange);
}

static BOOL isBlankDraft(NSString *text) {
    return !text || [text stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]].length == 0;
}

@interface ClawKitUiSnapshot : NSObject
@property (nonatomic) pid_t frontmostPid;
@property (nonatomic) BOOL claudeWasFrontmost;
@property (nonatomic, copy) NSString *focusedWindowTitle;
@property (nonatomic, copy) NSString *mainSessionPath;
@property (nonatomic) BOOL hasPrompt;
@property (nonatomic) BOOL promptFocused;
@property (nonatomic, copy) NSString *promptValue;
@property (nonatomic) BOOL hasRange;
@property (nonatomic) CFRange promptRange;
@end

@implementation ClawKitUiSnapshot
@end

// A successful press only means AX accepted the action. The Rust caller
// verifies persistence in Claude's own session record before sending.
static int pickMode(AXUIElementRef app, NSString *path) {
    NSArray *bypass = @[@"Bypass permissions", @"绕过权限", @"跳过权限", @"略過權限"];
    NSArray *modes = @[@"Accept edits", @"Ask permissions", @"Manual", @"Auto", @"Plan", @"Plan mode",
                       @"接受编辑", @"自动接受编辑", @"手动", @"自动", @"计划", @"接受編輯", @"手動", @"自動", @"計畫"];
    BOOL opened = NO, selected = NO, confirmed = NO;
    for (NSUInteger attempt = 0; attempt < 28; attempt++) {
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
                    if (AXUIElementPerformAction((__bridge AXUIElementRef)candidates.firstObject, kAXPressAction) != kAXErrorSuccess) return 3;
                    if (!opened) opened = YES;
                    else if (!selected) selected = YES;
                    else confirmed = YES;
                }
            }
        }
        [NSThread sleepForTimeInterval:0.09];
    }
    return opened ? 3 : 2;
}

static BOOL pressBackButton(AXUIElementRef app, id mainWin) {
    NSArray *backNames = @[@"Back", @"返回"];
    id page = findMainAppWebArea(app, NULL);
    for (id root in page ? @[page, mainWin ?: page] : (mainWin ? @[mainWin] : @[])) {
        NSArray *btns = matches(@[root], ^BOOL(AXUIElementRef e) {
            return roleIs(e, @"AXButton") && hasTitleOrDesc(e, backNames) &&
                   [attribute(e, kAXEnabledAttribute) boolValue];
        });
        if (btns.count == 1 &&
            AXUIElementPerformAction((__bridge AXUIElementRef)btns.firstObject, kAXPressAction) == kAXErrorSuccess) {
            return YES;
        }
    }
    return NO;
}

static BOOL waitForMainPath(AXUIElementRef app, NSString *expectedPath) {
    for (NSUInteger i = 0; i < 16; i++) {
        [NSThread sleepForTimeInterval:0.06];
        id p = findMainAppWebArea(app, NULL);
        NSURL *u = p ? attribute((__bridge AXUIElementRef)p, kAXURLAttribute) : nil;
        if ([u.path isEqual:expectedPath]) return YES;
    }
    return NO;
}

static void openDeepLinkBackground(NSString *url) {
    NSTask *task = [[NSTask alloc] init];
    task.launchPath = @"/usr/bin/open";
    task.arguments = @[@"-g", url];
    task.standardInput = [NSFileHandle fileHandleWithNullDevice];
    task.standardOutput = [NSFileHandle fileHandleWithNullDevice];
    task.standardError = [NSFileHandle fileHandleWithNullDevice];
    @try {
        [task launch];
        [task waitUntilExit];
    } @catch (NSException *ex) {
        (void)ex;
    }
}

void *clawkit_claude_capture_ui(void) {
    @autoreleasepool {
        if (!AXIsProcessTrusted()) return NULL;
        NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
        NSString *frontBundle = front.bundleIdentifier;
        if ([frontBundle isEqual:@"com.apple.loginwindow"] ||
            [frontBundle isEqual:@"com.apple.ScreenSaver.Engine"]) return NULL;
        NSArray *apps = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.anthropic.claudefordesktop"];
        if (apps.count != 1) return NULL;
        ClawKitUiSnapshot *snap = [[ClawKitUiSnapshot alloc] init];
        snap.frontmostPid = front ? front.processIdentifier : 0;
        snap.claudeWasFrontmost = [frontBundle isEqual:@"com.anthropic.claudefordesktop"];
        AXUIElementRef ax = AXUIElementCreateApplication([apps.firstObject processIdentifier]);
        AXUIElementSetMessagingTimeout(ax, 0.4);
        id focusedWin = attribute(ax, kAXFocusedWindowAttribute);
        if (focusedWin && roleIs((__bridge AXUIElementRef)focusedWin, @"AXWindow"))
            snap.focusedWindowTitle = attribute((__bridge AXUIElementRef)focusedWin, kAXTitleAttribute);
        id mainWin = nil, mainPage = findMainAppWebArea(ax, &mainWin);
        if (mainPage) {
            NSURL *url = attribute((__bridge AXUIElementRef)mainPage, kAXURLAttribute);
            if ([url isKindOfClass:[NSURL class]] && url.path.length > 0) snap.mainSessionPath = url.path;
        }
        id prompt = findPromptField(focusedWin ?: mainWin);
        if (prompt) {
            AXUIElementRef field = (__bridge AXUIElementRef)prompt;
            snap.hasPrompt = YES;
            snap.promptFocused = [attribute(field, kAXFocusedAttribute) boolValue];
            NSString *val = attribute(field, kAXValueAttribute);
            if ([val isKindOfClass:[NSString class]]) snap.promptValue = val;
            CFRange range = CFRangeMake(0, 0);
            if (readRange(field, &range)) { snap.hasRange = YES; snap.promptRange = range; }
        }
        CFRelease(ax);
        if (snap.frontmostPid == 0 && !snap.mainSessionPath && !snap.focusedWindowTitle) return NULL;
        return (__bridge_retained void *)snap;
    }
}

void clawkit_claude_free_ui(void *rawSnapshot) {
    if (!rawSnapshot) return;
    @autoreleasepool {
        ClawKitUiSnapshot *snap = (__bridge_transfer ClawKitUiSnapshot *)rawSnapshot;
        (void)snap;
    }
}

int clawkit_claude_restore_ui(void *rawSnapshot, const char *importedSessionId) {
    if (!rawSnapshot) return 0;
    @autoreleasepool {
        ClawKitUiSnapshot *snap = (__bridge_transfer ClawKitUiSnapshot *)rawSnapshot;
        if (!AXIsProcessTrusted()) return 1;
        NSArray *apps = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.anthropic.claudefordesktop"];
        if (apps.count != 1) return 2;
        AXUIElementRef ax = AXUIElementCreateApplication([apps.firstObject processIdentifier]);
        AXUIElementSetMessagingTimeout(ax, 0.5);

        NSString *importedPath = importedSessionId
            ? [@"/epitaxy/" stringByAppendingString:[NSString stringWithUTF8String:importedSessionId]] : nil;
        BOOL navigatedBack = NO;
        if (snap.mainSessionPath.length > 0 && ![snap.mainSessionPath isEqual:importedPath]) {
            id mainWin = nil;
            id curPage = findMainAppWebArea(ax, &mainWin);
            NSURL *curUrl = curPage ? attribute((__bridge AXUIElementRef)curPage, kAXURLAttribute) : nil;
            if (![curUrl.path isEqual:snap.mainSessionPath]) {
                BOOL reached = pressBackButton(ax, mainWin) && waitForMainPath(ax, snap.mainSessionPath);
                if (!reached && ([snap.mainSessionPath isEqual:@"/epitaxy"] || [snap.mainSessionPath hasPrefix:@"/epitaxy/"])) {
                    openDeepLinkBackground([@"claude://claude.ai" stringByAppendingString:snap.mainSessionPath]);
                    waitForMainPath(ax, snap.mainSessionPath);
                }
                navigatedBack = YES;
            }
        }
        if (navigatedBack) [NSThread sleepForTimeInterval:0.12];

        if (snap.claudeWasFrontmost) {
            id targetWin = nil;
            if (snap.focusedWindowTitle.length > 0 && ![snap.focusedWindowTitle isEqual:@"Claude"]) {
                for (id win in claudeWindows(ax)) {
                    if ([attribute((__bridge AXUIElementRef)win, kAXTitleAttribute) isEqual:snap.focusedWindowTitle]) {
                        targetWin = win;
                        break;
                    }
                }
                if (targetWin) {
                    AXUIElementRef tw = (__bridge AXUIElementRef)targetWin;
                    for (NSUInteger i = 0; i < 3; i++) {
                        AXUIElementPerformAction(tw, kAXRaiseAction);
                        AXUIElementSetAttributeValue(tw, kAXMainAttribute, kCFBooleanTrue);
                        AXUIElementSetAttributeValue(ax, kAXFocusedWindowAttribute, tw);
                        id cur = attribute(ax, kAXFocusedWindowAttribute);
                        if (cur && [attribute((__bridge AXUIElementRef)cur, kAXTitleAttribute) isEqual:snap.focusedWindowTitle]) break;
                        [NSThread sleepForTimeInterval:0.06];
                    }
                }
            } else {
                findMainAppWebArea(ax, &targetWin);
            }
            if (targetWin && snap.hasPrompt) {
                id prompt = nil;
                for (NSUInteger i = 0; i < 14 && !prompt; i++) {
                    prompt = findPromptField(targetWin);
                    if (!prompt) [NSThread sleepForTimeInterval:0.06];
                }
                if (prompt) {
                    AXUIElementRef field = (__bridge AXUIElementRef)prompt;
                    NSString *curVal = attribute(field, kAXValueAttribute);
                    if (!isBlankDraft(snap.promptValue) && isBlankDraft(curVal)) {
                        [NSThread sleepForTimeInterval:0.12];
                        curVal = attribute(field, kAXValueAttribute);
                        if (isBlankDraft(curVal)) {
                            AXUIElementSetAttributeValue(field, kAXValueAttribute, (__bridge CFTypeRef)snap.promptValue);
                            curVal = snap.promptValue;
                        }
                    }
                    if (snap.promptFocused) AXUIElementSetAttributeValue(field, kAXFocusedAttribute, kCFBooleanTrue);
                    if (snap.hasRange && [curVal isKindOfClass:[NSString class]]) {
                        CFIndex len = (CFIndex)curVal.length;
                        CFRange r = snap.promptRange;
                        if (snap.promptValue && curVal.length > snap.promptValue.length) {
                            r = CFRangeMake(len, 0);
                        } else {
                            if (r.location < 0) r.location = 0;
                            if (r.location > len) r.location = len;
                            if (r.length < 0 || r.location + r.length > len) r.length = 0;
                        }
                        AXValueRef rangeVal = AXValueCreate(kAXValueCFRangeType, &r);
                        if (rangeVal) {
                            AXUIElementSetAttributeValue(field, kAXSelectedTextRangeAttribute, rangeVal);
                            CFRelease(rangeVal);
                        }
                    }
                }
            }
        } else if (snap.frontmostPid > 0) {
            AXUIElementRef prevAx = AXUIElementCreateApplication(snap.frontmostPid);
            if (prevAx) {
                AXUIElementSetMessagingTimeout(prevAx, 0.3);
                AXUIElementSetAttributeValue(prevAx, kAXFrontmostAttribute, kCFBooleanTrue);
                CFRelease(prevAx);
            }
            NSRunningApplication *prev = [NSRunningApplication runningApplicationWithProcessIdentifier:snap.frontmostPid];
            if (prev && !prev.terminated) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                [prev activateWithOptions:NSApplicationActivateIgnoringOtherApps];
#pragma clang diagnostic pop
            }
        }
        CFRelease(ax);
        return 0;
    }
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
