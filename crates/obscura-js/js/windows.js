// Stitched into bootstrap.js's private closure by build.rs.
// Native registry IDs identify navigables, not realms: every operation resolves
// the current context and principal again so retained proxies survive navigation.
const _nativeWindowProxies = new Map();
const _nativeWindowParse = JSON.parse;
let _nativeWindowId = 0;
let _nativeWindowRoot = false;
let _nativeWindowRegistered = false;
let _nativeWindowMainRealm = false;
let _nativeFrameName = '';

function _windowAccessDenied() {
  throw new DOMException('Blocked access to a cross-origin window.', 'SecurityError');
}

function _windowRelation(id, name) {
  return _nativeWindowParse(__obscuraCore.ops.op_window_relation(id, name));
}

function _windowTarget(id) {
  const target = __obscuraCore.ops.op_window_target(id);
  if (!target) return _windowAccessDenied();
  return target;
}

function _windowSource(sourceRealmId) {
  const id = __obscuraCore.ops.op_window_source(sourceRealmId);
  if (!id) return null;
  return _windowProxy(id);
}

function _windowRelated(id, name) {
  const relation = _windowRelation(id, name);
  if (!relation.windowId) return null;
  const root = _windowRelation(relation.windowId, 'self');
  if (relation.realmId !== root.realmId) {
    if (relation.realmId === _realmFrameId) return globalThis;
    if (_realmFrameId && relation.realmId === _realmParentFrameId) return globalThis.parent;
    return _frameWindowFor(relation.realmId)
      || __obscuraCore.ops.op_window_realm_proxy(relation.windowId, relation.realmId);
  }
  return _windowProxy(relation.windowId);
}

function _windowPostMessage(id, data, targetOrigin, transfer) {
  const origin = _normalizePostMessageTargetOrigin(targetOrigin);
  if (targetOrigin !== null && (typeof targetOrigin === 'object' || typeof targetOrigin === 'function')) {
    transfer = targetOrigin.transfer;
  }
  __obscuraCore.ops.op_window_post_message(id, origin, data, transfer);
}

function _windowCrossOriginLocation(id) {
  const replace = _markNative(function replace(url) {
    __obscuraCore.ops.op_window_navigate(id, String(url), true);
  });
  return new Proxy(Object.create(null), {
    get(_target, key) {
      if (_iframeCrossOriginSpecialKey(key)) return undefined;
      if (key === 'replace') return replace;
      return _windowAccessDenied();
    },
    set(_target, key, value) {
      if (key !== 'href') return _windowAccessDenied();
      __obscuraCore.ops.op_window_navigate(id, String(value), false);
      return true;
    },
    has(_target, key) {
      if (key === 'href' || key === 'replace' || _iframeCrossOriginSpecialKey(key)) return true;
      return _windowAccessDenied();
    },
    ownKeys() { return ['href', 'replace']; },
    getOwnPropertyDescriptor(_target, key) {
      if (_iframeCrossOriginSpecialKey(key)) {
        return { value: undefined, writable: false, enumerable: false, configurable: true };
      }
      if (key === 'replace') return { value: replace, writable: false, enumerable: true, configurable: true };
      if (key === 'href') return {
        get: _windowAccessDenied,
        set(value) { __obscuraCore.ops.op_window_navigate(id, String(value), false); },
        enumerable: true, configurable: true,
      };
      return _windowAccessDenied();
    },
    defineProperty() { return _windowAccessDenied(); },
    deleteProperty() { return _windowAccessDenied(); },
    getPrototypeOf() { return null; },
    setPrototypeOf() { return _windowAccessDenied(); },
    preventExtensions() { return false; },
  });
}

function _windowProxy(id, realmId = undefined) {
  if (realmId !== undefined) {
    if (realmId === _realmFrameId) return globalThis;
    const frame = _frameWindowFor(realmId);
    if (frame) return frame;
    if (_windowRelation(id, 'self').realmId !== realmId) return null;
  }
  if (!id) return null;
  if (_nativeWindowRoot && id === _nativeWindowId) return globalThis;
  let proxy = _nativeWindowProxies.get(id);
  if (proxy) return proxy;
  const postMessage = _markNative(function postMessage(data, targetOrigin, transfer) {
    _windowPostMessage(id, data, targetOrigin, transfer);
  });
  const close = _markNative(function close() { __obscuraCore.ops.op_window_close(id); });
  const focus = _markNative(function focus() {});
  const blur = _markNative(function blur() {});
  const crossOriginLocation = _windowCrossOriginLocation(id);
  let nativeEval;
  function crossOriginValue(key) {
    if (_iframeCrossOriginSpecialKey(key)) return undefined;
    if (!_iframeCrossOriginKeySet.has(key)) return _windowAccessDenied();
    switch (key) {
      case 'window': case 'self': case 'frames': return proxy;
      case 'postMessage': return postMessage;
      case 'close': return close;
      case 'focus': return focus;
      case 'blur': return blur;
      case 'closed': return __obscuraCore.ops.op_window_closed(id);
      case 'location': return crossOriginLocation;
      case 'parent': case 'top': case 'opener': return _windowRelated(id, key);
      case 'length': return _windowRelation(id, 'length').length;
    }
  }
  function get(key) {
    if (key === 'closed') return __obscuraCore.ops.op_window_closed(id);
    if (key === 'postMessage') return postMessage;
    if (key === 'close') return close;
    if (!__obscuraCore.ops.op_window_same_origin(id)) return crossOriginValue(key);
    if (key === 'window' || key === 'self' || key === 'frames' || key === 'globalThis') return proxy;
    if (key === 'parent' || key === 'top' || key === 'opener') return _windowRelated(id, key);
    const real = _windowTarget(id);
    if (key === 'eval') {
      return nativeEval ||= _markNative(function(source) {
        const current = _windowTarget(id);
        const value = Reflect.apply(current.eval, current, [source]);
        return value === current ? proxy : value;
      });
    }
    const value = Reflect.get(real, key, real);
    return value === real ? proxy : value;
  }
  proxy = new Proxy(Object.create(null), {
    get(_target, key) { return get(key); },
    set(_target, key, value) {
      if (key === 'location') {
        __obscuraCore.ops.op_window_navigate(id, String(value), false);
        return true;
      }
      const real = _windowTarget(id);
      return Reflect.set(real, key, value, real);
    },
    has(_target, key) {
      if (!__obscuraCore.ops.op_window_same_origin(id)) {
        if (_iframeCrossOriginKeySet.has(key) || _iframeCrossOriginSpecialKey(key)) return true;
        return _windowAccessDenied();
      }
      return Reflect.has(_windowTarget(id), key);
    },
    ownKeys() {
      return __obscuraCore.ops.op_window_same_origin(id)
        ? Reflect.ownKeys(_windowTarget(id))
        : _iframeCrossOriginKeys.concat('then', Symbol.toStringTag, Symbol.hasInstance, Symbol.isConcatSpreadable);
    },
    getOwnPropertyDescriptor(_target, key) {
      if (!__obscuraCore.ops.op_window_same_origin(id)) {
        return { value: crossOriginValue(key), writable: false, enumerable: true, configurable: true };
      }
      const descriptor = Reflect.getOwnPropertyDescriptor(_windowTarget(id), key);
      if (!descriptor) return undefined;
      if ('value' in descriptor) return { ...descriptor, value: get(key), configurable: true };
      return {
        get: descriptor.get && function() { return get(key); },
        set: descriptor.set && function(value) { Reflect.set(proxy, key, value); },
        enumerable: descriptor.enumerable, configurable: true,
      };
    },
    defineProperty(_target, key, descriptor) {
      const real = _windowTarget(id);
      // The backing target stays extensible so navigation can replace its realm.
      if (descriptor.configurable !== true) return false;
      return Reflect.defineProperty(real, key, descriptor);
    },
    deleteProperty(_target, key) { return Reflect.deleteProperty(_windowTarget(id), key); },
    getPrototypeOf() {
      return __obscuraCore.ops.op_window_same_origin(id) ? Reflect.getPrototypeOf(_windowTarget(id)) : null;
    },
    setPrototypeOf(_target, prototype) { return Reflect.setPrototypeOf(_windowTarget(id), prototype); },
    preventExtensions() { return false; },
  });
  _nativeWindowProxies.set(id, proxy);
  return proxy;
}

function _initializeNativeWindow(child, windowId, realmId) {
  const core = child.__obscura_core_handoff;
  for (const name of Object.keys(__obscuraCore.ops)) core.ops[name] = __obscuraCore.ops[name];
  delete child.__obscura_core_handoff;
  delete child.Deno;
  for (const name of ['__obscura_ua', '__obscura_platform', '__obscura_ua_platform',
                     '__obscura_ua_platform_version', '__obscura_stealth',
                     '__obscura_geo_lat', '__obscura_geo_lon']) {
    if (globalThis[name] !== undefined) child[name] = globalThis[name];
  }
  child.__obscura_objects = {};
  child.__obscura_frameId = realmId;
  child.__obscura_parentFrameId = 0;
  child.__obscura_init(windowId);
  Object.defineProperties(child, {
    parent: { value: child, writable: false, enumerable: true, configurable: true },
    top: { value: child, writable: false, enumerable: true, configurable: true },
    frameElement: { value: null, writable: false, enumerable: true, configurable: true },
  });
}

function _deliverWindowMessage(data, origin, sourceWindowId, sourceRealmId) {
  const source = (!_nativeWindowRoot && sourceRealmId === _realmParentFrameId)
    ? globalThis.parent
    : (_frameWindowFor(sourceRealmId) || _windowSource(sourceRealmId)
      || __obscuraCore.ops.op_window_realm_proxy(sourceWindowId, sourceRealmId));
  _dispatchRealmMessage(data, origin, source);
}

function _registerNativeWindow(windowId) {
  if (_nativeWindowRegistered) return;
  _nativeWindowId = windowId || __obscuraCore.ops.op_window_current();
  const root = _windowRelation(_nativeWindowId, 'self');
  _nativeWindowRoot = root.realmId === _realmFrameId;
  _nativeWindowMainRealm = __obscuraCore.ops.op_window_register(
    _initializeNativeWindow, _windowProxy, _deliverWindowMessage);
  _nativeWindowRegistered = true;
}

Object.defineProperties(globalThis, {
  closed: {
    get() { return __obscuraCore.ops.op_window_closed(_nativeWindowId); },
    enumerable: true, configurable: true,
  },
  opener: {
    get() { return _nativeWindowRoot ? _windowRelated(_nativeWindowId, 'opener') : null; },
    set(value) {
      if (_nativeWindowRoot && value === null) __obscuraCore.ops.op_window_command(_nativeWindowId, 'opener-null', '');
    },
    enumerable: true, configurable: true,
  },
  name: {
    get() { return _nativeWindowRoot ? _windowRelation(_nativeWindowId, 'self').name : _nativeFrameName; },
    set(value) {
      if (_nativeWindowRoot) __obscuraCore.ops.op_window_command(_nativeWindowId, 'name', String(value));
      else _nativeFrameName = String(value);
    },
    enumerable: true, configurable: true,
  },
});
globalThis.open = _markNative(function open(url = '', target = '_blank', features = '') {
  const id = __obscuraCore.ops.op_window_open(String(url), String(target), String(features));
  return _windowProxy(id);
});
globalThis.close = _markNative(function close() {
  if (_nativeWindowRoot) __obscuraCore.ops.op_window_close(_nativeWindowId);
});
globalThis.postMessage = _markNative(function postMessage(data, targetOrigin, transfer) {
  if (_nativeWindowRoot) _windowPostMessage(_nativeWindowId, data, targetOrigin, transfer);
  else _sendRealmMessage(_realmFrameId, data, targetOrigin);
});
