#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import <sys/attr.h>
#import <fcntl.h>
#import <unistd.h>
#import <errno.h>
#import <string.h>
#import <stdlib.h>

typedef void (*MarkerupPickerCallback)(const char *, const unsigned char *, size_t, void *);
extern void markerup_ios_resume_request(void);

// ATTR_CMN_OBJTYPE returns the Darwin vnode type. The iOS SDK exposes the
// attribute but not the private sys/vnode.h constants: VREG is 1 and VDIR is 2.
static const uint32_t MarkerupVnodeRegularFile = 1;
static const uint32_t MarkerupVnodeDirectory = 2;

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

// Some SMB File Provider implementations return an empty result from
// FileManager's directory APIs even while Files can display the same folder.
// getattrlistbulk is a public iOS API that reads directory entries directly;
// Apple DTS recommends it as the workaround for this exact failure mode.
static NSArray<NSURL *> *MarkerupBulkDirectoryContents(NSURL *directory, NSError **errorOut) {
    const char *path = directory.fileSystemRepresentation;
    if (!path) {
        if (errorOut) *errorOut = [NSError errorWithDomain:@"Markerup" code:4 userInfo:@{
            NSLocalizedDescriptionKey: @"The provider directory does not have a filesystem path"
        }];
        return nil;
    }

    int directoryFD = open(path, O_RDONLY | O_DIRECTORY);
    if (directoryFD < 0) {
        if (errorOut) *errorOut = [NSError errorWithDomain:NSPOSIXErrorDomain code:errno userInfo:nil];
        return nil;
    }

    struct attrlist attributes;
    memset(&attributes, 0, sizeof(attributes));
    attributes.bitmapcount = ATTR_BIT_MAP_COUNT;
    attributes.commonattr = ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME |
                            ATTR_CMN_ERROR | ATTR_CMN_OBJTYPE;

    NSMutableArray<NSURL *> *children = [NSMutableArray array];
    size_t bufferSize = 32 * 1024;
    uint8_t *buffer = malloc(bufferSize);
    if (!buffer) {
        close(directoryFD);
        if (errorOut) *errorOut = [NSError errorWithDomain:NSPOSIXErrorDomain code:ENOMEM userInfo:nil];
        return nil;
    }
    NSError *enumerationError = nil;
    for (;;) {
        int count = getattrlistbulk(directoryFD, &attributes, buffer, bufferSize, FSOPT_PACK_INVAL_ATTRS);
        if (count == 0) break;
        if (count < 0) {
            enumerationError = [NSError errorWithDomain:NSPOSIXErrorDomain code:errno userInfo:nil];
            break;
        }

        uint8_t *entry = buffer;
        uint8_t *bufferEnd = buffer + bufferSize;
        for (int index = 0; index < count; index++) {
            if (entry + sizeof(uint32_t) > bufferEnd) {
                enumerationError = [NSError errorWithDomain:@"Markerup" code:5 userInfo:@{
                    NSLocalizedDescriptionKey: @"Malformed getattrlistbulk directory entry"
                }];
                break;
            }

            uint32_t entryLength = 0;
            memcpy(&entryLength, entry, sizeof(entryLength));
            if (entryLength < sizeof(uint32_t) + sizeof(attribute_set_t) || entry + entryLength > bufferEnd) {
                enumerationError = [NSError errorWithDomain:@"Markerup" code:5 userInfo:@{
                    NSLocalizedDescriptionKey: @"Malformed getattrlistbulk directory entry length"
                }];
                break;
            }

            uint8_t *entryEnd = entry + entryLength;
            uint8_t *field = entry + sizeof(uint32_t);
            attribute_set_t returned;
            memcpy(&returned, field, sizeof(returned));
            field += sizeof(returned);

            uint32_t itemError = 0;
            if (returned.commonattr & ATTR_CMN_ERROR) {
                if (field + sizeof(itemError) > entryEnd) { enumerationError = [NSError errorWithDomain:@"Markerup" code:5 userInfo:nil]; break; }
                memcpy(&itemError, field, sizeof(itemError));
                field += sizeof(itemError);
            }

            attrreference_t nameReference;
            if (!(returned.commonattr & ATTR_CMN_NAME) || field + sizeof(nameReference) > entryEnd) {
                entry = entryEnd;
                continue;
            }
            memcpy(&nameReference, field, sizeof(nameReference));
            if (nameReference.attr_dataoffset < 0 || nameReference.attr_length == 0 ||
                (size_t)nameReference.attr_dataoffset > (size_t)(entryEnd - field) ||
                (size_t)nameReference.attr_length > (size_t)(entryEnd - field - nameReference.attr_dataoffset)) {
                entry = entryEnd;
                continue;
            }
            uint8_t *nameBytes = field + nameReference.attr_dataoffset;
            field += sizeof(nameReference);
            if (itemError) {
                entry = entryEnd;
                continue;
            }

            uint32_t objectType = MarkerupVnodeRegularFile;
            if (returned.commonattr & ATTR_CMN_OBJTYPE) {
                if (field + sizeof(objectType) > entryEnd) { enumerationError = [NSError errorWithDomain:@"Markerup" code:5 userInfo:nil]; break; }
                memcpy(&objectType, field, sizeof(objectType));
            }

            size_t nameLength = (size_t)nameReference.attr_length;
            if (nameLength > 0 && nameBytes[nameLength - 1] == '\0') nameLength--;
            NSString *name = [[NSString alloc] initWithBytes:nameBytes length:nameLength encoding:NSUTF8StringEncoding];
            if (name.length > 0) {
                [children addObject:[directory URLByAppendingPathComponent:name isDirectory:(objectType == MarkerupVnodeDirectory)]];
            }
            entry = entryEnd;
        }
        if (enumerationError) break;
    }
    close(directoryFD);
    free(buffer);

    if (enumerationError) {
        if (errorOut) *errorOut = enumerationError;
        return nil;
    }
    return children;
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
            NSError *bulkError = nil;
            NSArray<NSURL *> *children = MarkerupBulkDirectoryContents(directory, &bulkError);
            // File Provider APIs remain the fallback for providers that do
            // not support getattrlistbulk. Also consult them after an empty
            // direct enumeration because Apple's r.150542999 can make the
            // first File Provider result falsely empty.
            if (!children || children.count == 0) {
                NSArray<NSURL *> *providerChildren = nil;
                NSError *providerError = nil;
                for (NSUInteger attempt = 0; attempt < 3; attempt++) {
                    providerError = nil;
                    providerChildren = [manager contentsOfDirectoryAtURL:directory
                                            includingPropertiesForKeys:@[NSURLNameKey, NSURLIsDirectoryKey, NSURLFileResourceTypeKey]
                                                               options:NSDirectoryEnumerationSkipsHiddenFiles
                                                                 error:&providerError];
                    if (!providerChildren || providerChildren.count > 0 || attempt == 2) break;
                    NSLog(@"Markerup: empty provider enumeration for %@; retrying (%lu)",
                          directory, (unsigned long)(attempt + 1));
                }
                if (providerChildren.count > 0 || !children) children = providerChildren;
                if (!children && providerError) error = providerError;
                if (!children && !error) error = bulkError;
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
