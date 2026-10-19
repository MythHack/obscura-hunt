// Native WindowProxy access-control seam for the snapshot global template.
//
// V8 only offers the named/indexed access-check interceptor on an
// ObjectTemplate.  The template is serialized into the startup snapshot, so
// this file is compiled into both the snapshot producer (build script process)
// and the target runtime.  The callback addresses are exposed through one
// ordered table; deno_core's extension external-reference list is built from
// the same table in both processes, which keeps the serialized external
// pointer indices identical.
//
// The per-context state is a private bundle stored in the global proxy's
// internal field 0 by bootstrap.  The bundle is created in the target realm,
// so no user-visible recursive proxy or foreign raw global is handed out.  A
// cross-origin property read therefore performs exactly the operations the
// target realm performs for itself while the V8 access-check boundary still
// controls which names can be reached.
//
// Browser contract implemented here (verified against Chromium):
//   - visible names: window, self, location, closed, frames, length, top,
//     opener, parent, blur, close, focus, postMessage, "then";
//   - visible well-known symbols: @@toStringTag, @@hasInstance,
//     @@isConcatSpreadable (value undefined, non-enumerable);
//   - ownKeys = live child frame indices + the names above + those symbols;
//   - Object.keys = enumerable indices only;
//   - hasProperty throws SecurityError for everything else;
//   - define/delete/setPrototypeOf/preventExtensions throw SecurityError;
//   - getPrototypeOf is null;
//   - location.href read throws, location.href write / replace navigate;
//   - no bundle means no permissive fallback: SecurityError in the accessing
//     realm except for the identity names and "then".

#include "v8.h"

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <memory>

namespace {

constexpr int kBundleInternalField = 0;
constexpr int kAccessInternalField = 1;
enum class WindowAccess { kDenied = 0, kSameWorld = 1, kDomGrant = 2 };
constexpr size_t kMaxIndexedKeys = 65536;

bool IsStringName(v8::Local<v8::Name> name) { return name->IsString(); }

// Accepts both literals and pointers into kCrossOriginNames (the literal
// template overload cannot deduce a length from `const char*`).
v8::Local<v8::String> Utf8Key(v8::Isolate* isolate, const char* literal) {
  return v8::String::NewFromUtf8(isolate, literal,
                                 v8::NewStringType::kInternalized, -1)
      .ToLocalChecked();
}

bool NameEquals(v8::Isolate* isolate, v8::Local<v8::Name> name,
                const char* literal) {
  if (!IsStringName(name)) return false;
  return name.As<v8::String>()
      ->Equals(isolate->GetCurrentContext(), Utf8Key(isolate, literal))
      .FromMaybe(false);
}

enum class CrossName : int {
  kWindow = 0,
  kSelf,
  kFrames,
  kLocation,
  kPostMessage,
  kClose,
  kFocus,
  kBlur,
  kClosed,
  kParent,
  kTop,
  kOpener,
  kLength,
  kThen,
  kUnknown,
};

struct NameEntry {
  const char* literal;
  CrossName name;
};

// Order is the Chromium cross-origin Window surface; it is also the order the
// enumerator reports so ownKeys matches the browser layout (indices first,
// then names, then symbols).
constexpr NameEntry kCrossOriginNames[] = {
    {"window", CrossName::kWindow},   {"self", CrossName::kSelf},
    {"location", CrossName::kLocation}, {"closed", CrossName::kClosed},
    {"frames", CrossName::kFrames},   {"length", CrossName::kLength},
    {"top", CrossName::kTop},         {"opener", CrossName::kOpener},
    {"parent", CrossName::kParent},   {"blur", CrossName::kBlur},
    {"close", CrossName::kClose},     {"focus", CrossName::kFocus},
    {"postMessage", CrossName::kPostMessage}, {"then", CrossName::kThen},
};

CrossName ClassifyName(v8::Isolate* isolate, v8::Local<v8::Name> name) {
  if (!IsStringName(name)) return CrossName::kUnknown;
  for (const NameEntry& entry : kCrossOriginNames) {
    if (NameEquals(isolate, name, entry.literal)) return entry.name;
  }
  return CrossName::kUnknown;
}

bool IsIdentityName(CrossName name) {
  return name == CrossName::kWindow || name == CrossName::kSelf ||
         name == CrossName::kFrames;
}

// Values are read from the target bundle (including functions and the
// Location facade); foreign callable values are adapted below. Callable
// relations are separate so their exceptions surface.
bool IsBundleValueName(CrossName name) {
  return name == CrossName::kLocation || name == CrossName::kPostMessage ||
         name == CrossName::kClose || name == CrossName::kFocus ||
         name == CrossName::kBlur;
}

bool IsCallableName(CrossName name) {
  return name == CrossName::kClosed || name == CrossName::kParent ||
         name == CrossName::kTop || name == CrossName::kOpener ||
         name == CrossName::kLength;
}

const char* BundleKey(CrossName name) {
  switch (name) {
    case CrossName::kLocation: return "location";
    case CrossName::kPostMessage: return "postMessage";
    case CrossName::kClose: return "close";
    case CrossName::kFocus: return "focus";
    case CrossName::kBlur: return "blur";
    case CrossName::kClosed: return "closed";
    case CrossName::kParent: return "parent";
    case CrossName::kTop: return "top";
    case CrossName::kOpener: return "opener";
    case CrossName::kLength: return "length";
    default: return nullptr;
  }
}

bool IsEnumeratedSymbol(v8::Isolate* isolate, v8::Local<v8::Name> name) {
  if (!name->IsSymbol()) return false;
  return name.As<v8::Value>()->StrictEquals(
             v8::Symbol::GetToStringTag(isolate)) ||
         name.As<v8::Value>()->StrictEquals(
             v8::Symbol::GetHasInstance(isolate)) ||
         name.As<v8::Value>()->StrictEquals(
             v8::Symbol::GetIsConcatSpreadable(isolate));
}

v8::Local<v8::Object> BundleFor(v8::Local<v8::Object> holder) {
  if (holder.IsEmpty()) return {};
  if (holder->InternalFieldCount() <= kBundleInternalField) return {};
  v8::Local<v8::Value> field =
      holder->GetInternalField(kBundleInternalField).As<v8::Value>();
  if (field->IsObject()) return field.As<v8::Object>();
  // The proxy owns the bundle, but tolerate builds where the middleware is
  // restored on the backing global object instead.
  v8::Local<v8::Value> prototype = holder->GetPrototype();
  if (prototype->IsObject()) {
    v8::Local<v8::Object> backing = prototype.As<v8::Object>();
    if (backing->InternalFieldCount() > kBundleInternalField) {
      field = backing->GetInternalField(kBundleInternalField).As<v8::Value>();
      if (field->IsObject()) return field.As<v8::Object>();
    }
  }
  return {};
}

// The per-context bundle plus the target realm needed to invoke it.
struct Target {
  v8::Local<v8::Object> window;
  v8::Local<v8::Object> bundle;
  v8::Local<v8::Context> context;
};


void CrossOriginCallableThunk(
    const v8::FunctionCallbackInfo<v8::Value>& info);
void ThrowIllegalInvocation(v8::Isolate* isolate);
void ThrowSecurityError(v8::Isolate* isolate, const Target& target,
                        v8::Local<v8::Value> property);

void ResolveTarget(v8::Local<v8::Object> holder, Target* target) {
  target->window = holder;
  // `context->Global()` is the WindowProxy even when the callback holder is
  // the backing global object.  A detached proxy keeps its identity and its
  // internal field, but has no creation context.
  v8::Local<v8::Context> context;
  if (holder->GetCreationContext().ToLocal(&context)) {
    v8::Local<v8::Object> global = context->Global();
    if (!global.IsEmpty()) target->window = global;
  }
  target->bundle = BundleFor(target->window);
  if (target->bundle.IsEmpty() && target->window != holder) {
    target->bundle = BundleFor(holder);
  }
  if (holder->GetCreationContext().ToLocal(&context)) {
    target->context = context;
  } else if (!target->bundle.IsEmpty() &&
             target->bundle->GetCreationContext().ToLocal(&context)) {
    // Closed/detached documents: the bundle itself still remembers the realm.
    target->context = context;
  }
}

// The callback is installed after restoration, outside the snapshot callback
// table. Its arguments come only from V8 contexts, never author-supplied IDs.
WindowAccess AccessFor(v8::Local<v8::Context> accessing_context,
                       const Target& target) {
  if (accessing_context.IsEmpty() || target.context.IsEmpty() ||
      target.window.IsEmpty() ||
      target.window->InternalFieldCount() <= kAccessInternalField) {
    return WindowAccess::kDenied;
  }
  v8::Local<v8::Value> callback =
      target.window->GetInternalField(kAccessInternalField).As<v8::Value>();
  if (!callback->IsFunction()) return WindowAccess::kDenied;
  v8::Local<v8::Value> argv[] = {accessing_context->Global(), target.window};
  v8::Local<v8::Value> result;
  if (!callback.As<v8::Function>()
           ->Call(target.context, target.window, 2, argv).ToLocal(&result) ||
      !result->IsInt32()) {
    return WindowAccess::kDenied;
  }
  const int access = result.As<v8::Int32>()->Value();
  return access == 1 ? WindowAccess::kSameWorld
       : access == 2 ? WindowAccess::kDomGrant : WindowAccess::kDenied;
}

v8::Local<v8::Function> BundleFunction(v8::Isolate* isolate,
                                       const Target& target,
                                       const char* name) {
  if (target.bundle.IsEmpty() || target.context.IsEmpty()) return {};
  v8::Local<v8::Value> value;
  if (!target.bundle->Get(target.context, Utf8Key(isolate, name))
           .ToLocal(&value) ||
      !value->IsFunction()) {
    return {};
  }
  return value.As<v8::Function>();
}

int CallableLength(v8::Isolate* isolate, const Target& target,
                   v8::Local<v8::Function> function) {
  v8::Local<v8::Value> value;
  if (!function->Get(target.context, Utf8Key(isolate, "length"))
           .ToLocal(&value)) {
    return 0;
  }
  int32_t length = 0;
  if (!value->Int32Value(target.context).To(&length) || length < 0) {
    return 0;
  }
  return length;
}

v8::Local<v8::Function> NewAccessRealmCallable(
    v8::Isolate* isolate, const Target& target, CrossName name,
    v8::Local<v8::Function> function) {
  v8::Local<v8::Context> current = isolate->GetCurrentContext();
  if (current.IsEmpty() || target.context.IsEmpty() || function.IsEmpty()) {
    return {};
  }
  Target accessing{current->Global(), BundleFor(current->Global()), current};
  v8::Local<v8::Function> cache =
      BundleFunction(isolate, accessing, "crossOriginCallable");
  if (cache.IsEmpty()) return {};
  v8::Local<v8::Value> cache_arguments[] = {
      target.bundle, v8::Integer::New(isolate, static_cast<int>(name)),
      v8::Undefined(isolate)};
  v8::Local<v8::Value> cached;
  if (!cache->Call(current, accessing.window, 3, cache_arguments)
           .ToLocal(&cached)) return {};
  if (cached->IsFunction()) return cached.As<v8::Function>();

  // Keep target edges in V8's heap, not persistent native roots: the private
  // WeakMap cache must not keep a retired document alive through its wrapper.
  v8::Local<v8::Value> fields[] = {
      target.window, target.bundle,
      v8::Integer::New(isolate, static_cast<int>(name))};
  v8::Local<v8::Array> data = v8::Array::New(isolate, fields, 3);
  v8::Local<v8::Function> wrapper;
  if (!v8::Function::New(current, CrossOriginCallableThunk, data,
                         CallableLength(isolate, target, function),
                         v8::ConstructorBehavior::kThrow)
           .ToLocal(&wrapper)) return {};
  v8::Local<v8::Value> function_name = function->GetName();
  if (function_name->IsString()) {
    wrapper->SetName(function_name.As<v8::String>());
  }
  cache_arguments[2] = wrapper;
  if (!cache->Call(current, accessing.window, 3, cache_arguments)
           .ToLocal(&cached)) return {};
  return wrapper;
}

enum class ValueStatus { kFound, kAbsent, kThrew };

ValueStatus BundleProperty(v8::Isolate* isolate, const Target& target,
                           CrossName name, v8::Local<v8::Value>* result) {
  const char* key = BundleKey(name);
  if (key == nullptr || target.bundle.IsEmpty() ||
      target.context.IsEmpty()) {
    return ValueStatus::kAbsent;
  }
  v8::Local<v8::Value> value;
  if (!target.bundle->Get(target.context, Utf8Key(isolate, key))
           .ToLocal(&value)) {
    return isolate->HasPendingException() ? ValueStatus::kThrew
                                          : ValueStatus::kAbsent;
  }
  if (value->IsFunction() &&
      AccessFor(isolate->GetCurrentContext(), target) ==
          WindowAccess::kDenied) {
    v8::Local<v8::Function> wrapper = NewAccessRealmCallable(
        isolate, target, name, value.As<v8::Function>());
    if (wrapper.IsEmpty()) {
      return isolate->HasPendingException() ? ValueStatus::kThrew
                                            : ValueStatus::kAbsent;
    }
    value = wrapper;
  }
  *result = value;
  return ValueStatus::kFound;
}

ValueStatus CallBundle(v8::Isolate* isolate, const Target& target,
                       CrossName name, v8::Local<v8::Value> receiver,
                       int argc, v8::Local<v8::Value> argv[],
                       v8::Local<v8::Value>* result) {
  const char* key = BundleKey(name);
  if (key == nullptr) return ValueStatus::kAbsent;
  v8::Local<v8::Function> function = BundleFunction(isolate, target, key);
  if (function.IsEmpty()) {
    return isolate->HasPendingException() ? ValueStatus::kThrew
                                          : ValueStatus::kAbsent;
  }
  v8::Local<v8::Value> value;
  if (!function->Call(target.context, receiver, argc, argv)
           .ToLocal(&value)) {
    return isolate->HasPendingException() ? ValueStatus::kThrew
                                          : ValueStatus::kAbsent;
  }
  *result = value;
  return ValueStatus::kFound;
}

void CrossOriginCallableThunk(
    const v8::FunctionCallbackInfo<v8::Value>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  v8::Local<v8::Context> current = isolate->GetCurrentContext();
  v8::Local<v8::Value> raw_data = info.Data();
  if (current.IsEmpty() || raw_data.IsEmpty() || !raw_data->IsArray()) {
    ThrowIllegalInvocation(isolate);
    return;
  }
  v8::Local<v8::Array> data = raw_data.As<v8::Array>();
  v8::Local<v8::Value> window, bundle, name;
  if (!data->Get(current, 0).ToLocal(&window) || !window->IsObject() ||
      !data->Get(current, 1).ToLocal(&bundle) || !bundle->IsObject() ||
      !data->Get(current, 2).ToLocal(&name) || !name->IsInt32()) {
    ThrowIllegalInvocation(isolate);
    return;
  }
  v8::Local<v8::Context> target_context;
  if (!bundle.As<v8::Object>()->GetCreationContext().ToLocal(&target_context)) {
    ThrowIllegalInvocation(isolate);
    return;
  }
  Target target{window.As<v8::Object>(), bundle.As<v8::Object>(), target_context};
  constexpr int kInlineArgumentCount = 8;
  v8::Local<v8::Value> inline_arguments[kInlineArgumentCount];
  std::unique_ptr<v8::Local<v8::Value>[]> heap_arguments;
  v8::Local<v8::Value>* arguments = inline_arguments;
  const int argc = info.Length();
  if (argc > kInlineArgumentCount) {
    heap_arguments = std::make_unique<v8::Local<v8::Value>[]>(argc);
    arguments = heap_arguments.get();
  }
  for (int i = 0; i < argc; ++i) arguments[i] = info[i];

  v8::Local<v8::Value> result;
  ValueStatus status = CallBundle(isolate, target,
      static_cast<CrossName>(name.As<v8::Int32>()->Value()), target.window,
      argc, arguments, &result);
  if (status == ValueStatus::kFound) {
    info.GetReturnValue().Set(result);
  } else if (status == ValueStatus::kAbsent) {
    ThrowSecurityError(
        isolate, target,
        Utf8Key(isolate, "window").As<v8::Value>());
  }
}

void ThrowNativeSecurityError(v8::Isolate* isolate,
                              v8::Local<v8::Context> context) {
  v8::Local<v8::Context> scope = context;
  if (scope.IsEmpty()) scope = isolate->GetCurrentContext();
  v8::Local<v8::Value> error = v8::Exception::Error(
      v8::String::NewFromUtf8Literal(
          isolate, "Blocked access to a cross-origin window."));
  if (error->IsObject() && !scope.IsEmpty()) {
    v8::Maybe<bool> ignored = error.As<v8::Object>()->Set(
        scope, v8::String::NewFromUtf8Literal(isolate, "name"),
        v8::String::NewFromUtf8Literal(isolate, "SecurityError"));
    (void)ignored;
  }
  isolate->ThrowException(error);
}

// Calls `deny`; returns true only when it left a pending exception.
bool CallDeny(v8::Isolate* isolate, const Target& target,
              v8::Local<v8::Object> bundle,
              v8::Local<v8::Context> context,
              v8::Local<v8::Value> property) {
  if (bundle.IsEmpty() || context.IsEmpty()) return false;
  v8::Local<v8::Value> value;
  if (!bundle->Get(context, Utf8Key(isolate, "deny")).ToLocal(&value) ||
      !value->IsFunction()) {
    return isolate->HasPendingException();
  }
  v8::Local<v8::Function> deny = value.As<v8::Function>();
  v8::Local<v8::Value> argv[] = {property};
  v8::Local<v8::Value> ignored;
  const bool completed =
      deny->Call(context, target.window, 1, argv).ToLocal(&ignored);
  return !completed && isolate->HasPendingException();
}

// Throws in the accessing realm so callers see their own DOMException
// prototype.  Falls back to the target realm, then to a native error whose
// `name` is SecurityError when no Window bundle exists.
void ThrowSecurityError(v8::Isolate* isolate, const Target& target,
                        v8::Local<v8::Value> property) {
  if (isolate->HasPendingException()) return;
  v8::Local<v8::Context> current = isolate->GetCurrentContext();
  if (!current.IsEmpty()) {
    v8::Local<v8::Object> current_global = current->Global();
    v8::Local<v8::Object> current_bundle = BundleFor(current_global);
    if (!current_bundle.IsEmpty()) {
      Target accessor_target{current_global, current_bundle, current};
      if (CallDeny(isolate, accessor_target, current_bundle, current,
                   property)) {
        return;
      }
    }
  }
  if (isolate->HasPendingException()) return;
  if (!target.bundle.IsEmpty() && !target.context.IsEmpty() &&
      CallDeny(isolate, target, target.bundle, target.context, property)) {
    return;
  }
  if (isolate->HasPendingException()) return;
  ThrowNativeSecurityError(isolate,
                           current.IsEmpty() ? target.context : current);
}

// Resolves one cross-origin-visible Window property.  Returns kAbsent for a
// name that is not visible; the caller turns that into a SecurityError.
ValueStatus ResolveNamed(v8::Isolate* isolate, const Target& target,
                         CrossName id, v8::Local<v8::Name> name,
                         bool allow_named_child,
                         v8::Local<v8::Value>* result) {
  if (IsIdentityName(id)) {
    *result = target.window;
    return ValueStatus::kFound;
  }
  if (id == CrossName::kThen) {
    *result = v8::Undefined(isolate);
    return ValueStatus::kFound;
  }
  if (!name.IsEmpty() && NameEquals(isolate, name, "document") &&
      AccessFor(isolate->GetCurrentContext(), target) ==
          WindowAccess::kDomGrant) {
    v8::Context::Scope entered(target.context);
    return target.window->Get(target.context, name).ToLocal(result)
               ? ValueStatus::kFound : ValueStatus::kThrew;
  }
  if (IsBundleValueName(id)) {
    return BundleProperty(isolate, target, id, result);
  }
  if (IsCallableName(id)) {
    return CallBundle(isolate, target, id, target.window, 0, nullptr, result);
  }
  // Unknown string names may still be named child browsing contexts.  The
  // bundle resolves those in the target realm; anything that is not an object
  // (verified again by the bundle) must not become visible.
  if (allow_named_child && !name.IsEmpty() && IsStringName(name) &&
      !target.bundle.IsEmpty() && !target.context.IsEmpty()) {
    v8::Local<v8::Function> named =
        BundleFunction(isolate, target, "named");
    if (!named.IsEmpty()) {
      v8::Local<v8::Value> argv[] = {name.As<v8::Value>()};
      v8::Local<v8::Value> value;
      if (!named->Call(target.context, target.window, 1, argv)
               .ToLocal(&value)) {
        return isolate->HasPendingException() ? ValueStatus::kThrew
                                              : ValueStatus::kAbsent;
      }
      // The bundle promises a child Window proxy/facade.  Reject any scalar
      // so a bundle bug cannot expose a DOM value across the boundary.
      if (value->IsObject()) {
        *result = value;
        return ValueStatus::kFound;
      }
    }
  }
  return ValueStatus::kAbsent;
}

// Writes through the cross-origin Location facade: its href setter performs
// the navigation, while every other Location member stays denied there.
ValueStatus WriteLocation(v8::Isolate* isolate, const Target& target,
                          v8::Local<v8::Value> value) {
  v8::Local<v8::Value> location;
  ValueStatus status =
      BundleProperty(isolate, target, CrossName::kLocation, &location);
  if (status != ValueStatus::kFound) return status;
  if (!location->IsObject()) return ValueStatus::kAbsent;
  bool wrote = false;
  if (!location.As<v8::Object>()
           ->Set(target.context, Utf8Key(isolate, "href"), value)
           .To(&wrote)) {
    return isolate->HasPendingException() ? ValueStatus::kThrew
                                          : ValueStatus::kAbsent;
  }
  return wrote ? ValueStatus::kFound : ValueStatus::kAbsent;
}

v8::Local<v8::Object> NewDescriptor(v8::Isolate* isolate) {
  return v8::Object::New(isolate);
}

void SetDescriptorField(v8::Isolate* isolate,
                        v8::Local<v8::Object> descriptor, const char* key,
                        v8::Local<v8::Value> value) {
  v8::Local<v8::Context> context = isolate->GetCurrentContext();
  if (context.IsEmpty()) return;
  v8::Maybe<bool> ignored =
      descriptor->Set(context, Utf8Key(isolate, key), value);
  (void)ignored;
}

void SetDataDescriptor(v8::Isolate* isolate, v8::Local<v8::Object> descriptor,
                       v8::Local<v8::Value> value, bool enumerable) {
  SetDescriptorField(isolate, descriptor, "value", value);
  SetDescriptorField(isolate, descriptor, "writable",
                     v8::Boolean::New(isolate, false));
  SetDescriptorField(isolate, descriptor, "enumerable",
                     v8::Boolean::New(isolate, enumerable));
  SetDescriptorField(isolate, descriptor, "configurable",
                     v8::Boolean::New(isolate, true));
}

void NamedValueGetterThunk(const v8::FunctionCallbackInfo<v8::Value>& info);
void LocationSetterThunk(const v8::FunctionCallbackInfo<v8::Value>& info);

bool SetAccessorDescriptor(v8::Isolate* isolate, const Target& target,
                           CrossName id, bool with_setter,
                           v8::Local<v8::Object> descriptor) {
  if (target.context.IsEmpty()) return false;
  v8::Local<v8::Context> current = isolate->GetCurrentContext();
  if (current.IsEmpty()) return false;
  v8::Local<v8::Context> creation_context = target.context;
  if (AccessFor(current, target) == WindowAccess::kDenied) {
    creation_context = current;
  }
  v8::Local<v8::Data> data =
      v8::Integer::New(isolate, static_cast<int>(id));
  v8::Local<v8::Function> getter;
  if (!v8::Function::New(creation_context, NamedValueGetterThunk, data)
           .ToLocal(&getter)) {
    return false;
  }
  SetDescriptorField(isolate, descriptor, "get", getter);
  if (with_setter) {
    v8::Local<v8::Function> setter;
    if (!v8::Function::New(creation_context, LocationSetterThunk, data)
             .ToLocal(&setter)) {
      return false;
    }
    SetDescriptorField(isolate, descriptor, "set", setter);
  }
  SetDescriptorField(isolate, descriptor, "enumerable",
                     v8::Boolean::New(isolate, false));
  SetDescriptorField(isolate, descriptor, "configurable",
                     v8::Boolean::New(isolate, true));
  return true;
}

// Resolves a callback target from a function receiver.  The target may have
// no bundle (internal context); identity names still resolve in that case.
void TargetFromThis(v8::Local<v8::Value> this_value, Target* target) {
  if (this_value->IsObject()) {
    ResolveTarget(this_value.As<v8::Object>(), target);
  } else {
    target->window = {};
    target->bundle = {};
    target->context = {};
  }
}

v8::Intercepted DispatchNamed(v8::Local<v8::Name> property,
                              const v8::PropertyCallbackInfo<v8::Value>& info,
                              bool descriptor) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  if (!IsStringName(property)) {
    if (IsEnumeratedSymbol(isolate, property)) {
      info.GetReturnValue().Set(v8::Undefined(isolate));
      return v8::Intercepted::kYes;
    }
    ThrowSecurityError(isolate, target, property.As<v8::Value>());
    return v8::Intercepted::kYes;
  }
  CrossName id = ClassifyName(isolate, property);
  v8::Local<v8::Value> value;
  ValueStatus status =
      ResolveNamed(isolate, target, id, property, !descriptor, &value);
  if (status == ValueStatus::kFound) {
    info.GetReturnValue().Set(value);
    return v8::Intercepted::kYes;
  }
  if (status == ValueStatus::kAbsent) {
    ThrowSecurityError(isolate, target, property.As<v8::Value>());
  }
  return v8::Intercepted::kYes;
}

v8::Intercepted NamedGetter(v8::Local<v8::Name> property,
                            const v8::PropertyCallbackInfo<v8::Value>& info) {
  return DispatchNamed(property, info, /*descriptor=*/false);
}

v8::Intercepted NamedSetter(v8::Local<v8::Name> property,
                            v8::Local<v8::Value> value,
                            const v8::PropertyCallbackInfo<v8::Boolean>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  const v8::Local<v8::Context> source = isolate->GetCurrentContext();
  if (IsStringName(property) && value->IsFunction() &&
      ClassifyName(isolate, property) == CrossName::kUnknown &&
      AccessFor(source, target) == WindowAccess::kDomGrant) {
    v8::Local<v8::Context> function_context;
    if (value.As<v8::Function>()->GetCreationContext().ToLocal(&function_context) &&
        function_context == source) {
      // A grant may publish its own callback, but never read or overwrite
      // author state, shadow a browser member, or publish a foreign function.
      v8::Context::Scope entered(target.context);
      bool exists;
      if (!target.window->Has(target.context, property).To(&exists)) {
        return v8::Intercepted::kYes;
      }
      if (!exists) {
        bool written;
        if (target.window->CreateDataProperty(target.context, property, value)
                .To(&written)) info.GetReturnValue().Set(written);
        return v8::Intercepted::kYes;
      }
    }
  }
  if (IsStringName(property) &&
      ClassifyName(isolate, property) == CrossName::kLocation) {
    ValueStatus status = WriteLocation(isolate, target, value);
    if (status == ValueStatus::kFound) {
      info.GetReturnValue().Set(true);
      return v8::Intercepted::kYes;
    }
    if (status == ValueStatus::kThrew) return v8::Intercepted::kYes;
  }
  ThrowSecurityError(isolate, target, property.As<v8::Value>());
  return v8::Intercepted::kYes;
}

v8::Intercepted NamedQuery(v8::Local<v8::Name> property,
                           const v8::PropertyCallbackInfo<v8::Integer>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  if (IsStringName(property)) {
    CrossName id = ClassifyName(isolate, property);
    if (NameEquals(isolate, property, "document") &&
        AccessFor(isolate->GetCurrentContext(), target) ==
            WindowAccess::kDomGrant) {
      info.GetReturnValue().Set(v8::Integer::New(isolate, v8::ReadOnly));
      return v8::Intercepted::kYes;
    }
    // All enumerated names are non-enumerable.  "then" reports DontEnum
    // rather than absent so Object.keys can filter it without throwing; that
    // is the one observable divergence from Chromium's `'then' in window`
    // SecurityError (its ownKeys still lists `then`).
    if (id != CrossName::kUnknown) {
      info.GetReturnValue().Set(
          v8::Integer::New(isolate, static_cast<int>(v8::DontEnum)));
      return v8::Intercepted::kYes;
    }
  } else if (IsEnumeratedSymbol(isolate, property)) {
    info.GetReturnValue().Set(
        v8::Integer::New(isolate, static_cast<int>(v8::DontEnum)));
    return v8::Intercepted::kYes;
  }
  ThrowSecurityError(isolate, target, property.As<v8::Value>());
  return v8::Intercepted::kYes;
}

v8::Intercepted NamedDeleter(
    v8::Local<v8::Name> property,
    const v8::PropertyCallbackInfo<v8::Boolean>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  ThrowSecurityError(isolate, target, property.As<v8::Value>());
  return v8::Intercepted::kYes;
}

v8::Intercepted NamedDefiner(
    v8::Local<v8::Name> property, const v8::PropertyDescriptor& descriptor,
    const v8::PropertyCallbackInfo<v8::Boolean>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  (void)descriptor;
  ThrowSecurityError(isolate, target, property.As<v8::Value>());
  return v8::Intercepted::kYes;
}

v8::Intercepted NamedDescriptor(
    v8::Local<v8::Name> property,
    const v8::PropertyCallbackInfo<v8::Value>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  v8::Local<v8::Object> descriptor = NewDescriptor(isolate);
  if (!IsStringName(property)) {
    if (IsEnumeratedSymbol(isolate, property)) {
      SetDataDescriptor(isolate, descriptor, v8::Undefined(isolate), false);
      info.GetReturnValue().Set(descriptor);
      return v8::Intercepted::kYes;
    }
    ThrowSecurityError(isolate, target, property.As<v8::Value>());
    return v8::Intercepted::kYes;
  }
  CrossName id = ClassifyName(isolate, property);
  if (id == CrossName::kThen) {
    SetDataDescriptor(isolate, descriptor, v8::Undefined(isolate), false);
    info.GetReturnValue().Set(descriptor);
    return v8::Intercepted::kYes;
  }
  if (id == CrossName::kUnknown) {
    v8::Local<v8::Value> value;
    ValueStatus status =
        ResolveNamed(isolate, target, id, property, true, &value);
    if (status == ValueStatus::kFound) {
      SetDataDescriptor(isolate, descriptor, value, false);
      info.GetReturnValue().Set(descriptor);
      return v8::Intercepted::kYes;
    }
    if (status == ValueStatus::kThrew) return v8::Intercepted::kYes;
    ThrowSecurityError(isolate, target, property.As<v8::Value>());
    return v8::Intercepted::kYes;
  }
  if (IsBundleValueName(id)) {
    v8::Local<v8::Value> value;
    ValueStatus status = BundleProperty(isolate, target, id, &value);
    if (status == ValueStatus::kFound) {
      SetDataDescriptor(isolate, descriptor, value, false);
      info.GetReturnValue().Set(descriptor);
      return v8::Intercepted::kYes;
    }
    if (status == ValueStatus::kThrew) return v8::Intercepted::kYes;
    ThrowSecurityError(isolate, target, property.As<v8::Value>());
    return v8::Intercepted::kYes;
  }
  if (!SetAccessorDescriptor(isolate, target, id,
                             id == CrossName::kLocation, descriptor)) {
    if (!isolate->HasPendingException()) {
      ThrowSecurityError(isolate, target, property.As<v8::Value>());
    }
    return v8::Intercepted::kYes;
  }
  info.GetReturnValue().Set(descriptor);
  return v8::Intercepted::kYes;
}

v8::Intercepted IndexedGetter(uint32_t index,
                              const v8::PropertyCallbackInfo<v8::Value>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  v8::Local<v8::Value> argv[] = {
      v8::Integer::NewFromUnsigned(isolate, index)};
  v8::Local<v8::Function> lookup = BundleFunction(isolate, target, "index");
  if (!lookup.IsEmpty()) {
    v8::Local<v8::Value> value;
    if (lookup->Call(target.context, target.window, 1, argv)
            .ToLocal(&value)) {
      if (value->IsObject()) {
        info.GetReturnValue().Set(value);
        return v8::Intercepted::kYes;
      }
    } else if (isolate->HasPendingException()) {
      return v8::Intercepted::kYes;
    }
  }
  ThrowSecurityError(isolate, target, argv[0]);
  return v8::Intercepted::kYes;
}

v8::Intercepted IndexedSetter(
    uint32_t index, v8::Local<v8::Value> value,
    const v8::PropertyCallbackInfo<v8::Boolean>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  char buffer[16];
  std::snprintf(buffer, sizeof(buffer), "%u", index);
  v8::Local<v8::String> property = v8::String::NewFromUtf8(
      isolate, buffer, v8::NewStringType::kInternalized).ToLocalChecked();
  (void)value;
  ThrowSecurityError(isolate, target, property);
  return v8::Intercepted::kYes;
}

v8::Intercepted IndexedDescriptor(
    uint32_t index, const v8::PropertyCallbackInfo<v8::Value>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  v8::Local<v8::Value> argv[] = {
      v8::Integer::NewFromUnsigned(isolate, index)};
  v8::Local<v8::Function> lookup = BundleFunction(isolate, target, "index");
  if (!lookup.IsEmpty()) {
    v8::Local<v8::Value> value;
    if (lookup->Call(target.context, target.window, 1, argv)
            .ToLocal(&value)) {
      if (value->IsObject()) {
        v8::Local<v8::Object> descriptor = NewDescriptor(isolate);
        SetDataDescriptor(isolate, descriptor, value, true);
        info.GetReturnValue().Set(descriptor);
        return v8::Intercepted::kYes;
      }
    } else if (isolate->HasPendingException()) {
      return v8::Intercepted::kYes;
    }
  }
  ThrowSecurityError(isolate, target, argv[0]);
  return v8::Intercepted::kYes;
}

void NamedEnumerator(const v8::PropertyCallbackInfo<v8::Array>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  v8::Local<v8::Context> context = isolate->GetCurrentContext();
  constexpr int kStringCount =
      static_cast<int>(sizeof(kCrossOriginNames) /
                       sizeof(kCrossOriginNames[0]));
  v8::Local<v8::Array> keys = v8::Array::New(isolate, kStringCount + 3);
  if (context.IsEmpty()) {
    info.GetReturnValue().Set(keys);
    return;
  }
  for (int i = 0; i < kStringCount; ++i) {
    v8::Maybe<bool> ignored = keys->Set(
        context, static_cast<uint32_t>(i),
        Utf8Key(isolate, kCrossOriginNames[i].literal));
    (void)ignored;
  }
  v8::Local<v8::Value> symbols[] = {
      v8::Symbol::GetToStringTag(isolate),
      v8::Symbol::GetHasInstance(isolate),
      v8::Symbol::GetIsConcatSpreadable(isolate),
  };
  for (int i = 0; i < 3; ++i) {
    v8::Maybe<bool> ignored = keys->Set(
        context, static_cast<uint32_t>(kStringCount + i), symbols[i]);
    (void)ignored;
  }
  info.GetReturnValue().Set(keys);
}

void IndexedEnumerator(const v8::PropertyCallbackInfo<v8::Array>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  Target target;
  ResolveTarget(info.Holder(), &target);
  v8::Local<v8::Context> context = isolate->GetCurrentContext();
  uint32_t length = 0;
  v8::Local<v8::Value> value;
  if (!target.bundle.IsEmpty() && !target.context.IsEmpty()) {
    ValueStatus status =
        CallBundle(isolate, target, CrossName::kLength, target.window, 0,
                   nullptr, &value);
    if (status == ValueStatus::kFound) {
      uint32_t converted = 0;
      if (value->Uint32Value(target.context).To(&converted)) {
        length = converted;
      }
    }
  }
  if (length > kMaxIndexedKeys) length = static_cast<uint32_t>(kMaxIndexedKeys);
  v8::Local<v8::Array> keys = v8::Array::New(isolate, static_cast<int>(length));
  if (context.IsEmpty()) {
    info.GetReturnValue().Set(keys);
    return;
  }
  for (uint32_t i = 0; i < length; ++i) {
    v8::Maybe<bool> ignored = keys->Set(
        context, i, v8::Integer::NewFromUnsigned(isolate, i));
    (void)ignored;
  }
  info.GetReturnValue().Set(keys);
}

void ThrowIllegalInvocation(v8::Isolate* isolate) {
  v8::Local<v8::Value> error = v8::Exception::TypeError(
      v8::String::NewFromUtf8Literal(isolate, "Illegal invocation"));
  isolate->ThrowException(error);
}

void NamedValueGetterThunk(const v8::FunctionCallbackInfo<v8::Value>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  v8::Local<v8::Context> current = isolate->GetCurrentContext();
  int raw_id = static_cast<int>(CrossName::kUnknown);
  if (info.This().IsEmpty() || current.IsEmpty() ||
      !info.Data()->Int32Value(current).To(&raw_id)) {
    ThrowIllegalInvocation(isolate);
    return;
  }
  CrossName id = static_cast<CrossName>(raw_id);
  Target target;
  TargetFromThis(info.This(), &target);
  v8::Local<v8::Value> value;
  ValueStatus status = ResolveNamed(isolate, target, id, {}, false, &value);
  if (status == ValueStatus::kFound) {
    info.GetReturnValue().Set(value);
    return;
  }
  if (status == ValueStatus::kAbsent) {
    ThrowSecurityError(
        isolate, target,
        Utf8Key(isolate, "window").As<v8::Value>());
  }
}

void LocationSetterThunk(const v8::FunctionCallbackInfo<v8::Value>& info) {
  v8::Isolate* isolate = info.GetIsolate();
  if (info.This().IsEmpty()) {
    ThrowIllegalInvocation(isolate);
    return;
  }
  Target target;
  TargetFromThis(info.This(), &target);
  v8::Local<v8::Value> value = info.Length() > 0
                                   ? info[0]
                                   : v8::Undefined(isolate).As<v8::Value>();
  ValueStatus status = WriteLocation(isolate, target, value);
  if (status == ValueStatus::kFound) {
    info.GetReturnValue().Set(true);
    return;
  }
  if (status == ValueStatus::kAbsent) {
    ThrowSecurityError(isolate, target,
                       Utf8Key(isolate, "location").As<v8::Value>());
  }
}

bool AccessCheck(v8::Local<v8::Context> accessing_context,
                 v8::Local<v8::Object> accessed_object,
                 v8::Local<v8::Value> data) {
  (void)data;
  Target target;
  ResolveTarget(accessed_object, &target);
  // A DOM-only grant deliberately remains behind the property interceptors.
  return AccessFor(accessing_context, target) == WindowAccess::kSameWorld;
}

void FailedAccessCheck(v8::Local<v8::Object> target, v8::AccessType type,
                       v8::Local<v8::Value> data) {
  // Non-property operations (`Object.setPrototypeOf`, `Object.freeze`,
  // `delete`, `Object.defineProperty`) report a failed access check here
  // instead of reaching the named/indexed interceptor.  Throw the accessing
  // realm's own SecurityError; V8 raises a TypeError if the callback returns
  // without throwing, which would be the wrong error type.
  (void)type;
  (void)data;
  v8::Isolate* isolate = v8::Isolate::GetCurrent();
  v8::HandleScope handle_scope(isolate);
  Target resolved;
  ResolveTarget(target, &resolved);
  ThrowSecurityError(isolate, resolved,
                     Utf8Key(isolate, "window").As<v8::Value>());
}

}  // namespace

extern "C" void obscura_v8_set_failed_access_check(v8::Isolate* isolate) {
  isolate->SetFailedAccessCheckCallbackFunction(FailedAccessCheck);
}

extern "C" void obscura_v8_detach_global(v8::Context* context) {
  context->DetachGlobal();
}

extern "C" void obscura_v8_install_window_access_check(
    v8::ObjectTemplate* object_template) {
  // The template pointer comes straight from Rust's `Local<ObjectTemplate>`.
  // No isolate or handle scope is needed here: the handler configurations
  // carry no realm data, and every callback receives its isolate from the
  // property callback info.
  //
  // Fields 0 and 1 hold the cross-origin bundle and runtime authorization
  // callback. Set the count before every producer serializes this template.
  object_template->SetInternalFieldCount(2);
  v8::NamedPropertyHandlerConfiguration named(
      NamedGetter, NamedSetter, NamedQuery, NamedDeleter, NamedEnumerator,
      NamedDefiner, NamedDescriptor,
      /*data=*/{},
      static_cast<v8::PropertyHandlerFlags>(
          v8::PropertyHandlerFlags::kNone));
  v8::IndexedPropertyHandlerConfiguration indexed(
      IndexedGetter, IndexedSetter,
      /*query=*/nullptr, /*deleter=*/nullptr, IndexedEnumerator,
      /*definer=*/nullptr, IndexedDescriptor,
      /*data=*/{},
      static_cast<v8::PropertyHandlerFlags>(
          v8::PropertyHandlerFlags::kNone));
  object_template->SetAccessCheckCallbackAndHandler(AccessCheck, named, indexed);
}

// The order of this table is the external-reference order.  Snapshot producer
// and runtime must call this exact function.  Existing indices 0..8 are
// stable; the three enumerator/descriptor callbacks are appended.
extern "C" void obscura_v8_window_callback_addresses(const void** out,
                                                    size_t count) {
  const void* addresses[] = {
      reinterpret_cast<const void*>(&AccessCheck),
      reinterpret_cast<const void*>(&NamedGetter),
      reinterpret_cast<const void*>(&NamedSetter),
      reinterpret_cast<const void*>(&NamedQuery),
      reinterpret_cast<const void*>(&NamedDeleter),
      reinterpret_cast<const void*>(&NamedDescriptor),
      reinterpret_cast<const void*>(&NamedDefiner),
      reinterpret_cast<const void*>(&IndexedGetter),
      reinterpret_cast<const void*>(&IndexedSetter),
      reinterpret_cast<const void*>(&NamedEnumerator),
      reinterpret_cast<const void*>(&IndexedDescriptor),
      reinterpret_cast<const void*>(&IndexedEnumerator),
  };
  constexpr size_t kAddressCount = sizeof(addresses) / sizeof(addresses[0]);
  for (size_t i = 0; i < count && i < kAddressCount; ++i) {
    out[i] = addresses[i];
  }
  for (size_t i = kAddressCount; i < count; ++i) {
    out[i] = nullptr;
  }
}

// Effective V8 ABI contract of this translation unit. build.rs defines the
// pointer-compression macros when deno_core selects that V8 configuration, so
// these are the same type-layout decisions used to compile every callback
// above. Snapshot producer and runtime compare this value before an isolate
// is created.
extern "C" uint32_t obscura_native_window_abi() {
  uint32_t flags = 0;
  if (v8::internal::PointerCompressionIsEnabled()) flags |= 1u << 0;
  if (v8::internal::SmiValuesAre31Bits()) flags |= 1u << 1;
  if (v8::internal::SandboxIsEnabled()) flags |= 1u << 2;
#ifdef V8_COMPRESS_POINTERS_IN_SHARED_CAGE
  flags |= 1u << 3;
#endif
#ifdef V8_CPPGC_MICROTASK_QUEUE
  flags |= 1u << 4;
#endif
  flags |= static_cast<uint32_t>(sizeof(void*)) << 8;
  return flags;
}
