#import "ObjCExceptionCatcher.h"

BOOL IVCatchException(void (^ NS_NOESCAPE block)(void)) {
    @try {
        block();
        return YES;
    }
    @catch (NSException *exception) {
        NSLog(@"[ImageVault] caught AppKit exception: %@ — %@\n%@",
              exception.name, exception.reason ?: @"(no reason)", exception.callStackSymbols);
        return NO;
    }
}
