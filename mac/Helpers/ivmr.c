// Image Vault — now-playing artwork helper.
//
// Since macOS 15.4, MediaRemote only answers Apple-signed callers. Apple's own
// /usr/bin/perl is one, so the app loads this tiny library into perl via
// DynaLoader and calls iv_artwork(), which writes the current artwork to
// $IV_ART_OUT and prints "ok" / "noart". It does nothing else.
#include <dlfcn.h>
#include <dispatch/dispatch.h>
#include <CoreFoundation/CoreFoundation.h>
#include <stdio.h>
#include <stdlib.h>

typedef void (*GetInfoFn)(dispatch_queue_t, void (^)(CFDictionaryRef));

void iv_artwork(void *perl, void *cv) {
  (void)perl; (void)cv;
  void *handle = dlopen("/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote", RTLD_NOW);
  GetInfoFn get = handle ? (GetInfoFn)dlsym(handle, "MRMediaRemoteGetNowPlayingInfo") : NULL;
  const char *out = getenv("IV_ART_OUT");
  if (!get || !out) { printf("!error\n"); fflush(stdout); return; }
  dispatch_semaphore_t done = dispatch_semaphore_create(0);
  __block int ok = 0;
  get(dispatch_get_global_queue(0, 0), ^(CFDictionaryRef info) {
    if (info) {
      CFDataRef data = CFDictionaryGetValue(info, CFSTR("kMRMediaRemoteNowPlayingInfoArtworkData"));
      if (data && CFGetTypeID(data) == CFDataGetTypeID() && CFDataGetLength(data) > 0) {
        FILE *f = fopen(out, "wb");
        if (f) { fwrite(CFDataGetBytePtr(data), 1, (size_t)CFDataGetLength(data), f); fclose(f); ok = 1; }
      }
    }
    dispatch_semaphore_signal(done);
  });
  dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
  printf(ok ? "ok\n" : "noart\n");
  fflush(stdout);
}
