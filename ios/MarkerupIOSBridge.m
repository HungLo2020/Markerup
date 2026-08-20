#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

typedef void (*MarkerupPickerCallback)(const char *, const unsigned char *, size_t, void *);
extern void markerup_ios_resume_request(void);

static NSMutableDictionary<NSString *, NSURL *> *MarkerupAccessMap(void) {
    static NSMutableDictionary<NSString *, NSURL *> *map;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ map = [NSMutableDictionary dictionary]; });
    return map;
}

@interface MarkerupPickerDelegate : NSObject <UIDocumentPickerDelegate>
@property(nonatomic, assign) MarkerupPickerCallback callback;
@property(nonatomic, assign) void *context;
@end

@implementation MarkerupPickerDelegate
- (void)documentPicker:(UIDocumentPickerViewController *)controller didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSURL *url = urls.firstObject;
    if (!url || ![url startAccessingSecurityScopedResource]) {
        self.callback(NULL, NULL, 0, self.context);
        return;
    }
    NSError *error = nil;
    NSData *bookmark = [url bookmarkDataWithOptions:NSURLBookmarkCreationWithSecurityScope
                     includingResourceValuesForKeys:nil relativeToURL:nil error:&error];
    if (error || !bookmark) {
        [url stopAccessingSecurityScopedResource];
        self.callback(NULL, NULL, 0, self.context);
        return;
    }
    @synchronized (MarkerupAccessMap()) { MarkerupAccessMap()[url.path] = url; }
    self.callback(url.path.UTF8String, bookmark.bytes, bookmark.length, self.context);
}
- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller {
    self.callback(NULL, NULL, 0, self.context);
}
@end

static UIViewController *MarkerupRootViewController(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:UIWindowScene.class]) continue;
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            if (window.isKeyWindow && window.rootViewController) return window.rootViewController;
        }
    }
    return UIApplication.sharedApplication.keyWindow.rootViewController;
}

void markerup_ios_present_directory_picker(MarkerupPickerCallback callback, void *context) {
    dispatch_async(dispatch_get_main_queue(), ^{
        UIViewController *root = MarkerupRootViewController();
        if (!root) { callback(NULL, NULL, 0, context); return; }
        MarkerupPickerDelegate *delegate = [MarkerupPickerDelegate new];
        delegate.callback = callback;
        delegate.context = context;
        UIDocumentPickerViewController *picker = [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:@[UTTypeFolder] asCopy:NO];
        picker.delegate = delegate;
        picker.allowsMultipleSelection = NO;
        [root presentViewController:picker animated:YES completion:nil];
        objc_setAssociatedObject(picker, "markerup_delegate", delegate, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    });
}

bool markerup_ios_resolve_bookmark(const unsigned char *bytes, size_t length, char **path_out) {
    NSData *data = [NSData dataWithBytes:bytes length:length];
    BOOL stale = NO;
    NSError *error = nil;
    NSURL *url = [NSURL URLByResolvingBookmarkData:data
                                           options:NSURLBookmarkResolutionWithSecurityScope
                                     relativeToURL:nil
                               bookmarkDataIsStale:&stale
                                             error:&error];
    if (!url || error || stale || ![url startAccessingSecurityScopedResource]) return false;
    const char *path = url.path.UTF8String;
    if (!path) { [url stopAccessingSecurityScopedResource]; return false; }
    *path_out = strdup(path);
    if (*path_out) { @synchronized (MarkerupAccessMap()) { MarkerupAccessMap()[url.path] = url; } }
    return *path_out != NULL;
}

void markerup_ios_free_string(char *path) { free(path); }

void markerup_ios_stop_access(const char *path) {
    if (!path) return;
    NSString *key = [NSString stringWithUTF8String:path];
    NSURL *url = nil;
    @synchronized (MarkerupAccessMap()) {
        url = MarkerupAccessMap()[key];
        [MarkerupAccessMap() removeObjectForKey:key];
    }
    [url stopAccessingSecurityScopedResource];
}

bool markerup_ios_read_file(const char *path, unsigned char **data_out, size_t *length_out) {
    NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
    __block NSData *contents = nil;
    __block NSError *error = nil;
    NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
    [coordinator coordinateReadingItemAtURL:url options:0 error:&error byAccessor:^(NSURL *coordinatedURL) {
        contents = [NSData dataWithContentsOfURL:coordinatedURL options:0 error:&error];
    }];
    if (error || !contents) return false;
    *length_out = contents.length;
    *data_out = malloc(contents.length);
    if (!*data_out && contents.length != 0) return false;
    memcpy(*data_out, contents.bytes, contents.length);
    return true;
}

bool markerup_ios_write_file(const char *path, const unsigned char *data, size_t length) {
    NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
    NSData *contents = [NSData dataWithBytes:data length:length];
    __block NSError *error = nil;
    NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
    [coordinator coordinateWritingItemAtURL:url options:0 error:&error byAccessor:^(NSURL *coordinatedURL) {
        [contents writeToURL:coordinatedURL options:NSDataWritingAtomic error:&error];
    }];
    return error == nil;
}

void markerup_ios_free_data(unsigned char *data, size_t length) {
    (void)length;
    free(data);
}

bool markerup_ios_mutate(const char *path, const char *destination, unsigned char operation, const unsigned char *data, size_t length) {
    NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
    NSURL *destinationURL = destination ? [NSURL fileURLWithPath:[NSString stringWithUTF8String:destination]] : nil;
    __block NSError *error = nil;
    NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
    [coordinator coordinateWritingItemAtURL:url options:0 error:&error byAccessor:^(NSURL *coordinatedURL) {
        NSFileManager *manager = NSFileManager.defaultManager;
        switch (operation) {
            case 0: {
                [manager createDirectoryAtURL:coordinatedURL withIntermediateDirectories:NO attributes:nil error:&error];
                break;
            }
            case 1: {
                NSData *contents = [NSData dataWithBytes:data length:length];
                [contents writeToURL:coordinatedURL options:NSDataWritingAtomic error:&error];
                break;
            }
            case 2:
                [manager moveItemAtURL:coordinatedURL toURL:destinationURL error:&error];
                break;
            case 3:
                [manager removeItemAtURL:coordinatedURL error:&error];
                break;
            default:
                error = [NSError errorWithDomain:@"Markerup" code:1 userInfo:nil];
        }
    }];
    return error == nil;
}

void markerup_ios_install_lifecycle_observers(void) {
    [NSNotificationCenter.defaultCenter addObserverForName:UIApplicationDidBecomeActiveNotification
                                                     object:nil
                                                      queue:NSOperationQueue.mainQueue
                                                 usingBlock:^(NSNotification *note) {
        markerup_ios_resume_request();
    }];
}
