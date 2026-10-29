#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Runs `block` and returns YES, or catches an Objective-C exception, logs it
/// and returns NO. Swift cannot catch NSException, and AppKit raises one if a
/// file-promise provider misbehaves (e.g. `receivePromisedFiles…` asserting on
/// an empty promised URL). Without this the whole app aborts (SIGABRT).
BOOL IVCatchException(void (^ NS_NOESCAPE block)(void));

NS_ASSUME_NONNULL_END
