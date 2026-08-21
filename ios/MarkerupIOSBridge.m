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
    // iOS document-picker URLs already carry the required implicit security
    // scope. iOS does not expose the macOS WithSecurityScope bookmark flags;
    // Apple's directory-access sample persists these URLs as minimal bookmarks.
    NSData *bookmark = [url bookmarkDataWithOptions:NSURLBookmarkCreationMinimalBookmark
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
    return nil;
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
                                           options:0
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

// Keep using the original security-scoped provider URL whenever possible.
// Reconstructing a plain file URL from url.path loses File Provider/SMB URL
// semantics even though the path looks equivalent.
static NSURL *MarkerupURLForPath(const char *path, BOOL isDirectory) {
    if (!path) return nil;
    NSString *key = [NSString stringWithUTF8String:path];
    if (!key) return nil;

    @synchronized (MarkerupAccessMap()) {
        NSURL *exact = MarkerupAccessMap()[key];
        if (exact) return exact;

        NSURL *bestURL = nil;
        NSString *bestKey = nil;
        for (NSString *candidate in MarkerupAccessMap()) {
            if (candidate.length >= key.length || ![key hasPrefix:candidate]) continue;
            if ([key characterAtIndex:candidate.length] != '/') continue;
            if (!bestKey || candidate.length > bestKey.length) {
                bestKey = candidate;
                bestURL = MarkerupAccessMap()[candidate];
            }
        }
        if (bestURL && bestKey) {
            NSString *relative = [key substringFromIndex:bestKey.length + 1];
            return [bestURL URLByAppendingPathComponent:relative isDirectory:isDirectory];
        }
    }

    return [NSURL fileURLWithPath:key isDirectory:isDirectory];
}

bool markerup_ios_read_file(const char *path, unsigned char **data_out, size_t *length_out) {
    NSURL *url = MarkerupURLForPath(path, NO);
    if (!url) return false;
    __block NSData *contents = nil;
    __block NSError *error = nil;
    NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
    [coordinator coordinateReadingItemAtURL:url options:NSFileCoordinatorReadingWithoutChanges error:&error byAccessor:^(NSURL *coordinatedURL) {
        BOOL hasSecurityScope = [coordinatedURL startAccessingSecurityScopedResource];
        contents = [NSData dataWithContentsOfURL:coordinatedURL options:0 error:&error];
        if (hasSecurityScope) [coordinatedURL stopAccessingSecurityScopedResource];
    }];
    if (error || !contents) {
        NSLog(@"Markerup: failed to read %@: %@", url, error);
        return false;
    }
    *length_out = contents.length;
    *data_out = malloc(contents.length);
    if (!*data_out && contents.length != 0) return false;
    memcpy(*data_out, contents.bytes, contents.length);
    return true;
}

bool markerup_ios_write_file(const char *path, const unsigned char *data, size_t length) {
    NSURL *url = MarkerupURLForPath(path, NO);
    if (!url) return false;
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
    NSURL *url = MarkerupURLForPath(path, NO);
    NSURL *destinationURL = destination ? MarkerupURLForPath(destination, NO) : nil;
    if (!url || (destination && !destinationURL)) return false;
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

static NSString *MarkerupEscapedRelativePath(NSString *path) {
    return [path stringByAddingPercentEncodingWithAllowedCharacters:[NSCharacterSet alphanumericCharacterSet]];
}

bool markerup_ios_list_entries(const char *path, unsigned char **data_out, size_t *length_out) {
    if (!path || !data_out || !length_out) return false;
    NSURL *root = MarkerupURLForPath(path, YES);
    if (!root) return false;
    __block NSMutableString *serialized = [NSMutableString string];
    __block NSError *error = nil;
    NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
    [coordinator coordinateReadingItemAtURL:root options:NSFileCoordinatorReadingWithoutChanges error:&error byAccessor:^(NSURL *coordinatedURL) {
        // Some remote File Providers do not implement NSDirectoryEnumerator
        // correctly for network-backed folders. Read one directory at a time
        // instead, retaining the provider URL returned by each operation.
        NSMutableArray<NSURL *> *directories = [NSMutableArray arrayWithObject:coordinatedURL];
        NSMutableArray<NSString *> *relativeDirectories = [NSMutableArray arrayWithObject:@""];
        NSFileManager *manager = NSFileManager.defaultManager;
        while (directories.count > 0 && !error) {
            NSURL *directory = directories.lastObject;
            NSString *relativeDirectory = relativeDirectories.lastObject;
            [directories removeLastObject];
            [relativeDirectories removeLastObject];
            BOOL directoryScope = [directory startAccessingSecurityScopedResource];
            NSArray<NSURL *> *children = nil;
            // iOS has a known File Provider bug where the first enumeration
            // of a remote SMB directory can report an empty result without
            // an error. Apple DTS documents that an immediate second
            // enumeration returns the actual contents (r.150542999).
            for (NSUInteger attempt = 0; attempt < 3; attempt++) {
                NSError *directoryError = nil;
                children = [manager contentsOfDirectoryAtURL:directory
                                      includingPropertiesForKeys:@[NSURLNameKey, NSURLIsDirectoryKey, NSURLFileResourceTypeKey]
                                                         options:NSDirectoryEnumerationSkipsHiddenFiles
                                                           error:&directoryError];
                if (!children || children.count > 0 || attempt == 2) {
                    error = directoryError;
                    break;
                }
                NSLog(@"Markerup: empty provider enumeration for %@; retrying (%lu)",
                      directory, (unsigned long)(attempt + 1));
            }
            if (!children) {
                if (directoryScope) [directory stopAccessingSecurityScopedResource];
                break;
            }

            for (NSURL *item in children) {
                BOOL itemScope = [item startAccessingSecurityScopedResource];
                NSString *name = nil;
                NSNumber *isDirectory = nil;
                NSString *resourceType = nil;
                NSError *resourceError = nil;
                // File Provider URLs are not ordinary file URLs. Use the
                // provider's resource name instead of parsing lastPathComponent,
                // and use its file-resource type when directory metadata is
                // unavailable (both cases occur with SMB providers).
                [item getResourceValue:&name forKey:NSURLNameKey error:&resourceError];
                [item getResourceValue:&isDirectory forKey:NSURLIsDirectoryKey error:&resourceError];
                [item getResourceValue:&resourceType forKey:NSURLFileResourceTypeKey error:&resourceError];
                if (name.length == 0) name = item.lastPathComponent;
                if (name.length == 0) {
                    error = resourceError ?: [NSError errorWithDomain:@"Markerup" code:3 userInfo:@{
                        NSLocalizedDescriptionKey: [NSString stringWithFormat:@"Provider returned a child without a name: %@", item]
                    }];
                    if (itemScope) [item stopAccessingSecurityScopedResource];
                    break;
                }

                if ([name isEqualToString:@"."] || [name isEqualToString:@".."] ||
                    [name containsString:@"/"] || [name containsString:@"\\"]) {
                    error = [NSError errorWithDomain:@"Markerup" code:2 userInfo:@{
                        NSLocalizedDescriptionKey: [NSString stringWithFormat:@"Provider returned an invalid child URL: %@", item]
                    }];
                    if (itemScope) [item stopAccessingSecurityScopedResource];
                    break;
                }

                BOOL directoryValue = isDirectory.boolValue;
                if ([resourceType isEqualToString:NSURLFileResourceTypeDirectory]) {
                    directoryValue = YES;
                } else if ([resourceType isEqualToString:NSURLFileResourceTypeRegular] ||
                           [resourceType isEqualToString:NSURLFileResourceTypeSymbolicLink]) {
                    directoryValue = NO;
                }

                NSString *relative = relativeDirectory.length > 0
                    ? [relativeDirectory stringByAppendingPathComponent:name]
                    : name;
                if (directoryValue) {
                    if (![name hasPrefix:@"."]) {
                        [serialized appendFormat:@"D:%@\n", MarkerupEscapedRelativePath(relative)];
                        [directories addObject:item];
                        [relativeDirectories addObject:relative];
                    }
                } else if ([name.lowercaseString hasSuffix:@".md"]) {
                    [serialized appendFormat:@"F:%@\n", MarkerupEscapedRelativePath(relative)];
                }
                if (itemScope) [item stopAccessingSecurityScopedResource];
            }
            if (directoryScope) [directory stopAccessingSecurityScopedResource];
        }
    }];
    if (error) {
        NSLog(@"Markerup: failed to enumerate workspace %@: %@", root, error);
        return false;
    }
    NSData *bytes = [serialized dataUsingEncoding:NSUTF8StringEncoding];
    *length_out = bytes.length;
    *data_out = malloc(bytes.length == 0 ? 1 : bytes.length);
    if (!*data_out) return false;
    if (bytes.length > 0) memcpy(*data_out, bytes.bytes, bytes.length);
    return true;
}

void markerup_ios_install_lifecycle_observers(void) {
    [NSNotificationCenter.defaultCenter addObserverForName:UIApplicationDidBecomeActiveNotification
                                                     object:nil
                                                 queue:NSOperationQueue.mainQueue
                                                 usingBlock:^(NSNotification *note) {
        (void)note;
        markerup_ios_resume_request();
    }];
}
