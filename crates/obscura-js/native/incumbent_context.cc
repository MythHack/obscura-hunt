// The include directory is the unmodified v8-isolate.h dependency closure from
// rusty_v8 150.4.0 (V8 15.0.245.2). See v8-150.4.0/LICENSE for its BSD license.
// Refresh these headers together with the version guard in build.rs when V8
// changes. No generated GN header or private engine source is needed here.
#include "v8-isolate.h"

// The isolate must be live and entered on its owning thread, with a caller-owned
// HandleScope. Do not introduce a nested HandleScope: the returned local belongs
// to the caller's scope, just like rusty_v8's GetCurrentContext binding.
extern "C" const v8::Context* obscura_v8_incumbent_context(v8::Isolate* isolate) {
  auto c = isolate->GetIncumbentContext();
  return c.IsEmpty() ? nullptr : *c;
}
