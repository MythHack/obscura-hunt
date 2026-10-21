// Stitched into bootstrap.js's private closure by build.rs.
// Native registry IDs identify navigables, not realms: every operation resolves
// the current context and principal again so retained proxies survive navigation.
const _nativeWindowParse = JSON.parse;
let _nativeWindowId = 0;
let _nativeWindowRoot = false;
let _nativeWindowRegistered = false;
let _nativeFrameName = '';

function _windowAccessDenied() {
  throw new DOMException('Blocked access to a cross-origin window.', 'SecurityError');
}

function _windowRelation(id, name, realmId = undefined) {
  return _nativeWindowParse(__obscuraCore.ops.op_window_relation(id, name, realmId));
}

function _windowSource(sourceRealmId) {
  const id = __obscuraCore.ops.op_window_source(sourceRealmId);
  if (!id) return null;
  return __obscuraCore.ops.op_window_realm_proxy(id, sourceRealmId);
}

// Relations always publish the stable native proxy; native access checks gate it.
function _windowOpenerTarget(relation) {
  if (!relation || !relation.windowId) return null;
  return __obscuraCore.ops.op_window_realm_proxy(relation.windowId, relation.realmId) || null;
}

function _windowOpener() {
  return _windowOpenerTarget(_windowRelation(_nativeWindowId, 'opener'));
}

function _windowRelated(id, name, realmId = undefined) {
  const relation = _windowRelation(id, name, realmId);
  if (!relation.windowId) return null;
  return __obscuraCore.ops.op_window_realm_proxy(relation.windowId, relation.realmId);
}

function _windowPostMessage(id, data, targetOrigin, transfer) {
  const origin = _normalizePostMessageTargetOrigin(targetOrigin);
  if (targetOrigin !== null && (typeof targetOrigin === 'object' || typeof targetOrigin === 'function')) {
    transfer = targetOrigin.transfer;
  }
  __obscuraCore.ops.op_window_post_message(id, origin, _encodeCloneValue(data), transfer);
}

// A native postMessage function keeps the public `postMessage` shape: the raw
// op's own name (`op_post_message`) would expose internals to function
// fingerprinting. Marking the value does not wrap it, so the call itself stays
// a native entry with the author's realm as the incumbent.
function _nativePostMessage(fn) {
  if (!_nativeStr.has(fn)) {
    try {
      Object.defineProperty(fn, 'name', { value: 'postMessage', configurable: true });
    } catch (_) {}
    _markNativeAs(fn, 'function postMessage() { [native code] }');
  }
  return fn;
}

function _facadePostMessageOp(windowTarget, deliver, id) {
  return _nativePostMessage(
    __obscuraCore.ops.op_facade_post_message.bind(undefined, windowTarget, deliver, id));
}

function _windowCrossOriginLocation(id, realmId) {
  const replace = _markNative(function replace(url) {
    __obscuraCore.ops.op_window_navigate(id, String(url), true, realmId, 'GET', '');
  });
  return new Proxy(Object.create(null), {
    get(_target, key) {
      if (_iframeCrossOriginSpecialKey(key)) return undefined;
      if (key === 'replace') return replace;
      return _windowAccessDenied();
    },
    set(_target, key, value) {
      if (key !== 'href') return _windowAccessDenied();
      __obscuraCore.ops.op_window_navigate(id, String(value), false, realmId, 'GET', '');
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
        set(value) { __obscuraCore.ops.op_window_navigate(id, String(value), false, realmId, 'GET', ''); },
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


function _initializeNativeWindow(child, windowId, realmId) {
  const core = child.__obscura_core_handoff;
  for (const name of Object.keys(__obscuraCore.ops)) core.ops[name] = __obscuraCore.ops[name];
  delete child.__obscura_core_handoff;
  delete child.Deno;
  for (const name of ['__obscura_ua', '__obscura_platform', '__obscura_ua_platform',
                     '__obscura_ua_platform_version', '__obscura_stealth',
                     '__obscura_language', '__obscura_languages']) {
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
  // A realm id cannot name a top-level source: each browsing context roots its
  // own window at frame 0, so the opener's root document is indistinguishable
  // from this realm's parent frame by id alone. The native opener relation
  // (window plus the document that opened this one) is the authority, and its
  // canonical object is the same one the `opener` global resolves to, so a
  // same-origin sender compares equal to the real parent Window while a
  // foreign principal keeps the gated facade.
  const opener = (_nativeWindowRoot && sourceWindowId !== _nativeWindowId)
    ? _windowRelation(_nativeWindowId, 'opener') : null;
  const fromOpener = !!opener && opener.windowId === sourceWindowId
    && opener.realmId === sourceRealmId;
  const source = (!_nativeWindowRoot && sourceRealmId === _realmParentFrameId)
    ? globalThis.parent
    : (fromOpener ? _windowOpenerTarget(opener)
      : (_canonicalFrameWindowFor(sourceRealmId) || _windowSource(sourceRealmId)
        || __obscuraCore.ops.op_window_realm_proxy(sourceWindowId, sourceRealmId)));
  _dispatchRealmMessage(_reviveCloneValue(data), origin, source);
}

function _registerNativeWindow(windowId, canonical) {
  if (_nativeWindowRegistered) return;
  _nativeWindowId = windowId || __obscuraCore.ops.op_window_current();
  const root = _windowRelation(_nativeWindowId, 'self');
  _nativeWindowRoot = root.realmId === _realmFrameId;
  if (canonical) {
    __obscuraCore.ops.op_window_register(
      _initializeNativeWindow, _deliverWindowMessage);
  }
  _nativeWindowRegistered = true;
}

Object.defineProperties(globalThis, {
  closed: {
    get() {
      // A discarded frame browsing context reports closed. The native realm
      // registry is the authority: it answers 0 once the frame is retired.
      // Already-retained Document and function references stay readable -
      // nothing here blanks or retargets the old document.
      if (!_nativeWindowRoot && _realmFrameId) {
        return __obscuraCore.ops.op_window_source(_realmFrameId) === 0;
      }
      return __obscuraCore.ops.op_window_closed(_nativeWindowId);
    },
    enumerable: true, configurable: true,
  },
  opener: {
    get() { return _nativeWindowRoot ? _windowOpener() : null; },
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
  const id = __obscuraCore.ops.op_window_open(String(url), String(target), String(features), 'GET', '');
  if (!id) return null;
  // Every world receives the same native identity with live access checks.
  const relation = _windowRelation(id, 'self');
  return relation.windowId && relation.realmId !== undefined
    ? __obscuraCore.ops.op_window_realm_proxy(id, relation.realmId)
    : null;
});
globalThis.close = _markNative(function close() {
  if (_nativeWindowRoot) __obscuraCore.ops.op_window_close(_nativeWindowId);
});
function _postMessageDispatch(windowTarget, id, data, targetOrigin, transfer, deliver) {
  // Invoked by a native `postMessage` entry in the *source* realm.
  // Attribution happens natively before this point; the enqueue ops below see
  // this realm's bootstrap frames as incumbent, never a receiver wrapper.
  if (!deliver) {
    // The facade's browsing context is gone. A browser still validates the
    // overload and serializes the value before discarding the message.
    _normalizePostMessageTargetOrigin(targetOrigin);
    if (!windowTarget) _serializeRealmMessage(data);
    return;
  }
  if (windowTarget) _windowPostMessage(id, data, targetOrigin, transfer);
  else _sendRealmMessage(id, data, targetOrigin);
}
